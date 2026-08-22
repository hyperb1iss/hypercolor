//! Background system sensor polling for the render pipeline.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use tokio::sync::watch;
use tracing::debug;

use hypercolor_types::sensor::{SensorReading, SensorUnit, SystemSnapshot};
use sysinfo::{
    Components, CpuRefreshKind, MINIMUM_CPU_UPDATE_INTERVAL, MemoryRefreshKind, RefreshKind, System,
};

use hypercolor_worker_retention::{retain_worker, spawn_worker};

use super::traits::{
    DataSource, DataSourceKind, DataSourceRole, InputData, InputSource, SourceRoleBinding,
};
use super::{SourceIssue, SourceKind, SourceStatusHandle, SourceStatusReporter};

const DEFAULT_SENSOR_POLL_INTERVAL: Duration = Duration::from_secs(2);
const SENSOR_STOP_TIMEOUT: Duration = Duration::from_secs(1);
const BYTES_PER_MEGABYTE: f64 = 1_000_000.0;

/// Graph-owned system telemetry source.
pub struct SensorSource {
    poller: SensorPoller,
    receiver: watch::Receiver<Arc<SystemSnapshot>>,
    running: bool,
    status: SourceStatusReporter,
}

impl SensorSource {
    /// Create a source using the native two-second acquisition cadence.
    #[must_use]
    pub fn new() -> Self {
        Self::with_interval(DEFAULT_SENSOR_POLL_INTERVAL)
    }

    fn with_interval(interval: Duration) -> Self {
        let poller = SensorPoller::with_interval(interval);
        let receiver = poller.receiver();
        Self {
            poller,
            receiver,
            running: false,
            status: SourceStatusReporter::new(
                "sensors",
                SourceKind::Sensors,
                "sysinfo",
                true,
                true,
                true,
            ),
        }
    }

    fn report_worker_exit(&mut self, reason: String) -> anyhow::Error {
        self.running = false;
        if let Some(status) = self.status.session() {
            status.failed(SourceIssue::new(
                "sensor_poller_exited",
                reason.clone(),
                true,
            ));
        }
        anyhow::anyhow!(reason)
    }

    #[cfg(test)]
    fn set_test_sampler(&mut self, sampler: impl FnMut() -> SystemSnapshot + Send + 'static) {
        self.poller.set_test_sampler(sampler);
    }
}

impl Default for SensorSource {
    fn default() -> Self {
        Self::new()
    }
}

impl InputSource for SensorSource {
    fn name(&self) -> &'static str {
        "sensors"
    }

    fn start(&mut self) -> Result<()> {
        if self.running {
            return Ok(());
        }
        self.status.begin_session()?;
        self.receiver = self.poller.receiver();
        if let Err(error) = self.poller.start() {
            if let Some(status) = self.status.session() {
                status.failed(SourceIssue::new(
                    "sensor_poller_start_failed",
                    error.to_string(),
                    true,
                ));
            }
            return Err(error);
        }
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
        self.poller.stop();
        self.status.stop();
    }

    fn sample(&mut self) -> Result<InputData> {
        if !self.running {
            return Ok(InputData::None);
        }
        if let Some(reason) = self.poller.observe_exit() {
            return Err(self.report_worker_exit(reason));
        }
        match self.receiver.has_changed() {
            Ok(false) => Ok(InputData::None),
            Ok(true) => {
                let snapshot = Arc::clone(&self.receiver.borrow_and_update());
                if snapshot.polled_at_ms == 0 {
                    return Ok(InputData::None);
                }
                if let Some(status) = self.status.session() {
                    let sampled_at = std::time::Instant::now();
                    status.record_sample(
                        sampled_at,
                        sampled_at + self.poller.interval.saturating_add(self.poller.interval),
                        1,
                    )?;
                }
                Ok(InputData::Sensors(snapshot))
            }
            Err(error) => {
                Err(self.report_worker_exit(format!("sensor publication channel closed: {error}")))
            }
        }
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }
}

impl SourceRoleBinding for SensorSource {
    type Role = DataSourceRole;
}

impl DataSource for SensorSource {
    fn data_source_kind(&self) -> DataSourceKind {
        DataSourceKind::Sensors
    }
}

/// Backoff between attempts to open the Windows PawnIO CPU temperature
/// reader while it is unavailable. Long enough that a permanently
/// PawnIO-less host pays almost nothing, short enough that installing
/// hardware support feels immediate rather than requiring a restart.
#[cfg(target_os = "windows")]
const CPU_TEMP_REPROBE_INTERVAL: Duration = Duration::from_secs(20);

/// Background poller that publishes latest-value system telemetry snapshots.
struct SensorPoller {
    interval: Duration,
    tx: watch::Sender<Arc<SystemSnapshot>>,
    publication_session: Arc<Mutex<Option<u64>>>,
    next_session_generation: u64,
    thread: Option<SensorPollerThread>,
    #[cfg(test)]
    sampler: Option<Box<dyn FnMut() -> SystemSnapshot + Send>>,
}

struct SensorPollerThread {
    stop_tx: Sender<()>,
    exit_rx: Receiver<()>,
    join_handle: JoinHandle<()>,
}

impl SensorPoller {
    /// Create a new poller using the default cadence.
    #[must_use]
    fn new() -> Self {
        Self::with_interval(DEFAULT_SENSOR_POLL_INTERVAL)
    }

    /// Create a new poller with a custom cadence.
    #[must_use]
    fn with_interval(interval: Duration) -> Self {
        let (tx, _) = watch::channel(Arc::new(SystemSnapshot::empty()));
        Self {
            interval: interval.max(MINIMUM_CPU_UPDATE_INTERVAL),
            tx,
            publication_session: Arc::new(Mutex::new(None)),
            next_session_generation: 0,
            thread: None,
            #[cfg(test)]
            sampler: None,
        }
    }

    /// Subscribe to latest-value snapshots.
    #[must_use]
    fn receiver(&self) -> watch::Receiver<Arc<SystemSnapshot>> {
        self.tx.subscribe()
    }

    /// Start the poller thread if it is not already running.
    ///
    /// # Errors
    ///
    /// Returns an error if the poller thread cannot be spawned.
    fn start(&mut self) -> Result<()> {
        if self.thread.is_some() {
            return Ok(());
        }

        let interval = self.interval;
        let tx = self.tx.clone();
        self.next_session_generation = self
            .next_session_generation
            .checked_add(1)
            .expect("sensor poller session generation exhausted");
        let session_generation = self.next_session_generation;
        {
            let mut active = self
                .publication_session
                .lock()
                .expect("sensor publication session lock is not poisoned");
            *active = Some(session_generation);
            tx.send_replace(Arc::new(SystemSnapshot::empty()));
        }
        let publication_session = Arc::clone(&self.publication_session);
        let (stop_tx, stop_rx) = mpsc::channel();
        let (exit_tx, exit_rx) = mpsc::sync_channel(1);
        #[cfg(test)]
        let mut sampler = self.sampler.take();
        let join_handle = spawn_worker(
            std::thread::Builder::new().name("hypercolor-sensors".to_owned()),
            move || {
                #[cfg(test)]
                if let Some(ref mut sampler) = sampler {
                    loop {
                        if !publish_sensor_snapshot(
                            &publication_session,
                            session_generation,
                            &tx,
                            sampler(),
                        ) {
                            break;
                        }
                        match stop_rx.recv_timeout(interval) {
                            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                            Err(RecvTimeoutError::Timeout) => {}
                        }
                    }
                    let _ = exit_tx.send(());
                    return;
                }

                let mut sampler = SystemSampler::new();
                loop {
                    if !publish_sensor_snapshot(
                        &publication_session,
                        session_generation,
                        &tx,
                        sampler.sample_snapshot(),
                    ) {
                        break;
                    }
                    match stop_rx.recv_timeout(interval) {
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                }
                let _ = exit_tx.send(());
            },
        );
        let join_handle = match join_handle {
            Ok(join_handle) => join_handle,
            Err(error) => {
                self.end_publication_session();
                return Err(error).context("failed to spawn sensor poller thread");
            }
        };

        self.thread = Some(SensorPollerThread {
            stop_tx,
            exit_rx,
            join_handle,
        });
        Ok(())
    }

    /// Stop the poller thread if it is running.
    fn stop(&mut self) {
        self.stop_with_timeout(SENSOR_STOP_TIMEOUT);
    }

    fn stop_with_timeout(&mut self, timeout: Duration) {
        self.end_publication_session();
        let Some(thread) = self.thread.take() else {
            return;
        };

        let _ = thread.stop_tx.send(());
        let _ = thread.exit_rx.recv_timeout(timeout);
        if thread.join_handle.is_finished() {
            if let Err(error) = thread.join_handle.join() {
                debug!("sensor poller thread join failed: {error:?}");
            }
            return;
        }
        tracing::warn!("sensor poller did not stop before the deadline; retaining its join handle");
        retain_worker(thread.join_handle, "sensor poller");
    }

    fn observe_exit(&mut self) -> Option<String> {
        let thread = self.thread.as_ref()?;
        let exit = match thread.exit_rx.try_recv() {
            Ok(()) | Err(TryRecvError::Disconnected) => true,
            Err(TryRecvError::Empty) => thread.join_handle.is_finished(),
        };
        if !exit {
            return None;
        }
        self.end_publication_session();
        let thread = self.thread.take().expect("observed sensor thread exists");
        match thread.join_handle.join() {
            Ok(()) => Some("sensor poller exited unexpectedly".to_owned()),
            Err(_) => Some("sensor poller panicked".to_owned()),
        }
    }

    fn end_publication_session(&self) {
        let mut active = self
            .publication_session
            .lock()
            .expect("sensor publication session lock is not poisoned");
        if active.take().is_some() {
            self.tx.send_replace(Arc::new(SystemSnapshot::empty()));
        }
    }

    #[cfg(test)]
    fn set_test_sampler(&mut self, sampler: impl FnMut() -> SystemSnapshot + Send + 'static) {
        self.sampler = Some(Box::new(sampler));
    }
}

fn publish_sensor_snapshot(
    publication_session: &Mutex<Option<u64>>,
    session_generation: u64,
    tx: &watch::Sender<Arc<SystemSnapshot>>,
    snapshot: SystemSnapshot,
) -> bool {
    let active = publication_session
        .lock()
        .expect("sensor publication session lock is not poisoned");
    if *active != Some(session_generation) {
        return false;
    }
    tx.send_replace(Arc::new(snapshot));
    true
}

impl Default for SensorPoller {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SensorPoller {
    fn drop(&mut self) {
        self.stop();
    }
}

struct SystemSampler {
    system: System,
    components: Components,
    nvidia: Option<NvidiaTelemetry>,
    #[cfg(target_os = "windows")]
    windows: WindowsSensorExtras,
}

impl SystemSampler {
    fn new() -> Self {
        let mut system = System::new_with_specifics(
            RefreshKind::nothing()
                .with_memory(MemoryRefreshKind::nothing().with_ram().with_swap())
                .with_cpu(CpuRefreshKind::nothing().with_cpu_usage()),
        );
        system.refresh_memory();
        system.refresh_cpu_usage();

        let mut components = Components::new_with_refreshed_list();
        components.refresh(false);

        Self {
            system,
            components,
            nvidia: NvidiaTelemetry::new(),
            #[cfg(target_os = "windows")]
            windows: WindowsSensorExtras::new(),
        }
    }

    fn sample_snapshot(&mut self) -> SystemSnapshot {
        self.system.refresh_memory();
        self.system.refresh_cpu_usage();
        self.components.refresh(false);

        let cpu_loads = self
            .system
            .cpus()
            .iter()
            .map(sysinfo::Cpu::cpu_usage)
            .collect::<Vec<_>>();
        let total_memory_mb = bytes_to_megabytes(self.system.total_memory());
        let used_memory_mb = bytes_to_megabytes(self.system.used_memory());
        let ram_used_percent = if total_memory_mb <= f64::EPSILON {
            0.0
        } else {
            ((used_memory_mb / total_memory_mb) * 100.0) as f32
        };
        let components = collect_component_readings(&self.components);
        let cpu_temp_celsius = best_cpu_temperature(&self.components);

        let mut snapshot = SystemSnapshot {
            cpu_load_percent: self.system.global_cpu_usage(),
            cpu_loads,
            cpu_temp_celsius,
            gpu_temp_celsius: best_gpu_temperature(&self.components),
            gpu_load_percent: None,
            gpu_vram_used_mb: None,
            ram_used_percent,
            ram_used_mb: used_memory_mb,
            ram_total_mb: total_memory_mb,
            components,
            polled_at_ms: unix_timestamp_ms(),
        };

        if let Some(nvidia) = self.nvidia.as_mut() {
            nvidia.merge_snapshot(&mut snapshot);
        }

        #[cfg(target_os = "windows")]
        self.windows.merge_snapshot(&mut snapshot);

        snapshot
    }
}

fn bytes_to_megabytes(bytes: u64) -> f64 {
    bytes as f64 / BYTES_PER_MEGABYTE
}

fn collect_component_readings(components: &Components) -> Vec<SensorReading> {
    components
        .iter()
        .filter_map(|component| {
            let temperature = component.temperature()?;
            if !temperature.is_finite() {
                return None;
            }

            Some(SensorReading::new(
                component.label().trim().to_owned(),
                temperature,
                SensorUnit::Celsius,
                None,
                component.max().filter(|value| value.is_finite()),
                component.critical().filter(|value| value.is_finite()),
            ))
        })
        .collect()
}

fn best_cpu_temperature(components: &Components) -> Option<f32> {
    find_temperature_by_priority(
        components,
        &[
            &["package", "cpu"],
            &["tctl"],
            &["tdie"],
            &["coretemp"],
            &["cpu"],
        ],
    )
}

fn best_gpu_temperature(components: &Components) -> Option<f32> {
    find_temperature_by_priority(
        components,
        &[&["gpu"], &["amdgpu"], &["radeon"], &["junction"], &["edge"]],
    )
}

fn find_temperature_by_priority(components: &Components, keyword_sets: &[&[&str]]) -> Option<f32> {
    for keywords in keyword_sets {
        if let Some(value) = components.iter().find_map(|component| {
            let label = component.label().to_ascii_lowercase();
            let temperature = component.temperature()?;
            if !temperature.is_finite() || !keywords.iter().all(|keyword| label.contains(keyword)) {
                return None;
            }
            Some(temperature)
        }) {
            return Some(value);
        }
    }

    None
}

fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

struct NvidiaTelemetry {
    nvml: nvml_wrapper::Nvml,
}

impl NvidiaTelemetry {
    fn new() -> Option<Self> {
        nvml_wrapper::Nvml::init().ok().map(|nvml| Self { nvml })
    }

    fn merge_snapshot(&mut self, snapshot: &mut SystemSnapshot) {
        use nvml_wrapper::enum_wrappers::device::TemperatureSensor;

        let Ok(device) = self.nvml.device_by_index(0) else {
            return;
        };

        if let Ok(temperature) = device.temperature(TemperatureSensor::Gpu) {
            snapshot.gpu_temp_celsius = Some(temperature as f32);
        }
        if let Ok(utilization) = device.utilization_rates() {
            snapshot.gpu_load_percent = Some(utilization.gpu as f32);
        }
        if let Ok(memory) = device.memory_info() {
            snapshot.gpu_vram_used_mb = Some(bytes_to_megabytes(memory.used) as f32);
        }
    }
}

#[cfg(target_os = "windows")]
#[allow(non_camel_case_types)]
#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct MSAcpi_ThermalZoneTemperature {
    current_temperature: u32,
    instance_name: String,
}

/// LibreHardwareMonitor / OpenHardwareMonitor share the same `Sensor` schema.
/// Both expose a `ROOT\LibreHardwareMonitor` or `ROOT\OpenHardwareMonitor`
/// namespace with one row per sensor. `SensorType` is a string enum:
/// `Temperature`, `Load`, `Clock`, `Voltage`, `Power`, `Fan`, `Data`,
/// `SmallData`, `Flow`, etc.
#[cfg(target_os = "windows")]
#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
struct HardwareMonitorSensor {
    name: String,
    value: f32,
    sensor_type: String,
    identifier: String,
}

#[cfg(target_os = "windows")]
const WBEM_E_ACCESS_DENIED_HRESULT: i32 = -2_147_217_405;

/// Windows-specific sensor extras. Builds a small cascade of sources at
/// startup and queries the available ones every poll:
///
/// 1. **PawnIO CPU thermal MSR/SMN reads** — first-class, no third-party
///    tools required. Uses the `IntelMSR.bin` / `AMDFamily17.bin` modules
///    we bundle with the installer. Read directly from `IA32_PACKAGE_THERM_STATUS`
///    on Intel and the `THM_TCON_CUR_TMP` SMN register on AMD Zen+.
/// 2. **LibreHardwareMonitor** (`ROOT\LibreHardwareMonitor.Sensor`) —
///    opportunistic; used if the user happens to be running it.
/// 3. **OpenHardwareMonitor** (`ROOT\OpenHardwareMonitor.Sensor`) — same
///    schema; older variant.
/// 4. **MSAcpi_ThermalZoneTemperature** (`ROOT\WMI`) — last-resort backfill
///    for systems with ACPI thermal zones but no PawnIO.
#[cfg(target_os = "windows")]
struct WindowsSensorExtras {
    cpu_temp: Option<hypercolor_windows_pawnio::CpuTempReader>,
    /// Set after we've emitted at least one warn about a PawnIO CPU temp
    /// failure, so the per-poll error doesn't spam the log every 2 seconds.
    cpu_temp_logged_error: bool,
    /// When to next attempt opening the PawnIO reader, while it is absent.
    cpu_temp_next_probe: Option<std::time::Instant>,
    libre_hardware: Option<wmi::WMIConnection>,
    open_hardware: Option<wmi::WMIConnection>,
    acpi_zones: Option<wmi::WMIConnection>,
    acpi_zones_enabled: bool,
}

#[cfg(target_os = "windows")]
impl WindowsSensorExtras {
    fn new() -> Self {
        // First-class CPU temp source: PawnIO MSR/SMN reads. Fails cleanly
        // if PawnIO isn't installed yet — the user will see CPU temp once
        // they accept the hardware-support prompt (which they need for
        // motherboard RGB anyway).
        let cpu_temp = match hypercolor_windows_pawnio::CpuTempReader::new() {
            Ok(reader) => {
                tracing::info!(
                    vendor = ?reader.vendor(),
                    "PawnIO CPU temperature reader online"
                );
                Some(reader)
            }
            Err(err) => {
                // Promoted from debug! — at INFO so it surfaces in normal
                // `just dev` runs. "PawnIO not installed yet" is the
                // expected reason on a fresh system; "module not found"
                // means the bundled .bin files weren't staged.
                tracing::info!(
                    "PawnIO CPU temperature reader unavailable ({err}); falling through to LHM/OHM/ACPI cascade"
                );
                None
            }
        };

        // Opportunistic sources — used if the user happens to be running
        // LibreHardwareMonitor or OpenHardwareMonitor, but never required.
        let libre_hardware = wmi::WMIConnection::with_namespace_path("ROOT\\LibreHardwareMonitor")
            .map_err(|err| debug!("LibreHardwareMonitor namespace not present: {err}"))
            .ok();
        let open_hardware = if libre_hardware.is_some() {
            None
        } else {
            wmi::WMIConnection::with_namespace_path("ROOT\\OpenHardwareMonitor")
                .map_err(|err| debug!("OpenHardwareMonitor namespace not present: {err}"))
                .ok()
        };
        let acpi_zones = wmi::WMIConnection::with_namespace_path("ROOT\\WMI")
            .map_err(|err| debug!("ROOT\\WMI namespace not present for ACPI thermal zones: {err}"))
            .ok();

        // Deliberately no early return when every source is absent: the
        // PawnIO reader is re-probed from merge_snapshot, and bailing here
        // would make that unreachable on the exact hosts that need it.
        let cpu_temp_next_probe = cpu_temp
            .is_none()
            .then(|| std::time::Instant::now() + CPU_TEMP_REPROBE_INTERVAL);

        Self {
            cpu_temp,
            cpu_temp_logged_error: false,
            cpu_temp_next_probe,
            libre_hardware,
            open_hardware,
            acpi_zones,
            acpi_zones_enabled: true,
        }
    }

    /// Retry opening the PawnIO reader once the backoff expires.
    ///
    /// The broker routinely arrives after the daemon does: the installer
    /// registers it while an upgrade's daemon is already running, PawnIO's
    /// kernel driver can need a reboot to bind to SCM, and Settings can
    /// install hardware support at any point. Probing once at startup left
    /// CPU temperature dead until someone thought to restart the daemon.
    fn reprobe_cpu_temp_if_due(&mut self) {
        let Some(next_probe) = self.cpu_temp_next_probe else {
            return;
        };
        if std::time::Instant::now() < next_probe {
            return;
        }

        match hypercolor_windows_pawnio::CpuTempReader::new() {
            Ok(reader) => {
                tracing::info!(
                    vendor = ?reader.vendor(),
                    "PawnIO CPU temperature reader came online"
                );
                self.cpu_temp = Some(reader);
                self.cpu_temp_logged_error = false;
                self.cpu_temp_next_probe = None;
            }
            Err(err) => {
                debug!("PawnIO CPU temperature reader still unavailable ({err})");
                self.cpu_temp_next_probe =
                    Some(std::time::Instant::now() + CPU_TEMP_REPROBE_INTERVAL);
            }
        }
    }

    fn merge_snapshot(&mut self, snapshot: &mut SystemSnapshot) {
        self.reprobe_cpu_temp_if_due();

        // 1. PawnIO MSR/SMN — first-class CPU temp source. Authoritative
        //    when it works; we won't override it with the WMI fallbacks below.
        if let Some(reader) = self.cpu_temp.as_mut() {
            match reader.read_package_celsius() {
                Ok(celsius) if celsius.is_finite() && (5.0..=125.0).contains(&celsius) => {
                    snapshot.cpu_temp_celsius = Some(celsius);
                    snapshot.components.push(SensorReading::new(
                        "cpu_package",
                        celsius,
                        SensorUnit::Celsius,
                        None,
                        None,
                        None,
                    ));
                    self.cpu_temp_logged_error = false;
                }
                Ok(celsius) => {
                    if !self.cpu_temp_logged_error {
                        tracing::warn!(
                            celsius,
                            "PawnIO CPU temp returned implausible value (expected 5..=125 °C); register decode is likely wrong"
                        );
                        self.cpu_temp_logged_error = true;
                    }
                }
                Err(err) => {
                    if !self.cpu_temp_logged_error {
                        tracing::warn!("PawnIO CPU temp read failed: {err}");
                        self.cpu_temp_logged_error = true;
                    }
                    // Drop the reader so the re-probe path can rebuild it.
                    // A stopped or crashed broker is otherwise permanent for
                    // the daemon's lifetime, and SCM restarting the broker
                    // underneath us should heal on its own.
                    self.cpu_temp = None;
                    self.cpu_temp_next_probe =
                        Some(std::time::Instant::now() + CPU_TEMP_REPROBE_INTERVAL);
                }
            }
        }

        // 2. LibreHardwareMonitor / OpenHardwareMonitor — only one runs at a time.
        if let Some(con) = self.libre_hardware.as_ref() {
            self.merge_hardware_monitor(con, snapshot, "lhm");
        } else if let Some(con) = self.open_hardware.as_ref() {
            self.merge_hardware_monitor(con, snapshot, "ohm");
        }

        // 3. ACPI thermal zones — additive context, only backfills cpu_temp
        //    if no better source produced one.
        if self.acpi_zones_enabled
            && let Some(con) = self.acpi_zones.as_ref()
        {
            let still_enabled = merge_acpi_thermal_zones(con, snapshot);
            self.acpi_zones_enabled = still_enabled;
        }
    }

    fn merge_hardware_monitor(
        &self,
        con: &wmi::WMIConnection,
        snapshot: &mut SystemSnapshot,
        label_prefix: &str,
    ) {
        let sensors: Vec<HardwareMonitorSensor> = match con.query() {
            Ok(rows) => rows,
            Err(err) => {
                debug!("hardware monitor sensor query failed: {err}");
                return;
            }
        };

        let mut best_cpu_temp: Option<f32> = None;
        let mut cpu_package_temp_seen = false;
        let mut best_gpu_temp: Option<f32> = None;
        let mut best_gpu_load: Option<f32> = None;

        for sensor in &sensors {
            let value = sensor.value;
            if !value.is_finite() {
                continue;
            }
            match sensor.sensor_type.as_str() {
                "Temperature" => {
                    if is_cpu_sensor(&sensor.identifier, &sensor.name) {
                        // Prefer "package" / "tdie" / "ccd" over per-core; otherwise take max.
                        if is_cpu_package(&sensor.name) {
                            best_cpu_temp = if cpu_package_temp_seen {
                                Some(best_cpu_temp.map_or(value, |cur| cur.max(value)))
                            } else {
                                Some(value)
                            };
                            cpu_package_temp_seen = true;
                        } else if !cpu_package_temp_seen {
                            best_cpu_temp = Some(best_cpu_temp.map_or(value, |cur| cur.max(value)));
                        }
                    } else if is_gpu_sensor(&sensor.identifier, &sensor.name)
                        && (is_gpu_core(&sensor.name) || best_gpu_temp.is_none())
                    {
                        best_gpu_temp = Some(value);
                    }
                    snapshot.components.push(SensorReading::new(
                        format!("{label_prefix}_{}", sanitize_zone_label(&sensor.identifier)),
                        value,
                        SensorUnit::Celsius,
                        None,
                        None,
                        None,
                    ));
                }
                "Load"
                    if is_gpu_sensor(&sensor.identifier, &sensor.name)
                        && is_gpu_core(&sensor.name) =>
                {
                    best_gpu_load = Some(value);
                }
                _ => {}
            }
        }

        if let Some(value) = best_cpu_temp {
            snapshot.cpu_temp_celsius = Some(value);
        }
        if let Some(value) = best_gpu_temp {
            // Only override if NVML didn't already produce one (NVML is more accurate
            // for NVIDIA cards; LHM may report driver-reported "Hot Spot" which is hotter).
            if snapshot.gpu_temp_celsius.is_none() {
                snapshot.gpu_temp_celsius = Some(value);
            }
        }
        if let Some(value) = best_gpu_load
            && snapshot.gpu_load_percent.is_none()
        {
            snapshot.gpu_load_percent = Some(value);
        }
    }
}

/// Returns `false` if the ACPI thermal zone source should be disabled for
/// the rest of the session (after access-denied or similar permanent
/// failure).
#[cfg(target_os = "windows")]
fn merge_acpi_thermal_zones(con: &wmi::WMIConnection, snapshot: &mut SystemSnapshot) -> bool {
    let zones: Vec<MSAcpi_ThermalZoneTemperature> = match con.query() {
        Ok(zones) => zones,
        Err(err) => {
            if wmi_access_denied(&err) {
                debug!("ACPI thermal zone query denied; disabling for this session");
                return false;
            }
            debug!("ACPI thermal zone query failed: {err}");
            return true;
        }
    };

    let mut max_celsius: Option<f32> = None;
    for zone in &zones {
        // NOTE: do NOT filter on `Active`. The ACPI `Active` field indicates
        // whether *active cooling has triggered*, not whether the reading is
        // valid. Many systems have permanently-inactive zones reporting
        // perfectly good motherboard / chipset temperatures.
        if zone.current_temperature == 0 {
            continue;
        }
        let celsius = deci_kelvin_to_celsius(zone.current_temperature);
        if !celsius.is_finite() || celsius <= 0.0 || celsius >= 150.0 {
            continue;
        }
        max_celsius = Some(match max_celsius {
            Some(current) => current.max(celsius),
            None => celsius,
        });
        snapshot.components.push(SensorReading::new(
            format!(
                "acpi_thermal_zone_{}",
                sanitize_zone_label(&zone.instance_name)
            ),
            celsius,
            SensorUnit::Celsius,
            None,
            None,
            None,
        ));
    }

    if snapshot.cpu_temp_celsius.is_none() {
        snapshot.cpu_temp_celsius = max_celsius;
    }
    true
}

#[cfg(target_os = "windows")]
fn wmi_access_denied(error: &wmi::WMIError) -> bool {
    match error {
        wmi::WMIError::HResultError { hres } => *hres == WBEM_E_ACCESS_DENIED_HRESULT,
        _ => false,
    }
}

/// LibreHardwareMonitor identifies CPU sensors with paths starting with
/// `/intelcpu/` or `/amdcpu/`. OpenHardwareMonitor uses the same convention.
#[cfg(target_os = "windows")]
fn is_cpu_sensor(identifier: &str, name: &str) -> bool {
    let ident = identifier.to_ascii_lowercase();
    ident.starts_with("/intelcpu/")
        || ident.starts_with("/amdcpu/")
        || ident.starts_with("/cpu/")
        || name.to_ascii_lowercase().starts_with("cpu ")
        || name.eq_ignore_ascii_case("cpu")
}

#[cfg(target_os = "windows")]
fn is_cpu_package(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("package")
        || lower.contains("tdie")
        || lower.contains("tctl")
        || lower.contains("ccd average")
        || lower == "cpu total"
}

#[cfg(target_os = "windows")]
fn is_gpu_sensor(identifier: &str, name: &str) -> bool {
    let ident = identifier.to_ascii_lowercase();
    ident.starts_with("/nvidiagpu/")
        || ident.starts_with("/atigpu/")
        || ident.starts_with("/amdgpu/")
        || ident.starts_with("/gpu/")
        || name.to_ascii_lowercase().starts_with("gpu ")
}

#[cfg(target_os = "windows")]
fn is_gpu_core(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("core") || lower.contains("die") || lower == "gpu"
}

/// ACPI thermal zones report `CurrentTemperature` in tenths of a Kelvin per
/// `MSAcpi_ThermalZoneTemperature` documentation. Convert to Celsius.
#[cfg(target_os = "windows")]
#[allow(clippy::as_conversions, clippy::cast_precision_loss)]
fn deci_kelvin_to_celsius(value: u32) -> f32 {
    (f64::from(value) / 10.0 - 273.15) as f32
}

/// Strip ACPI / HardwareMonitor path prefixes for clean labels.
#[cfg(target_os = "windows")]
fn sanitize_zone_label(instance_name: &str) -> String {
    instance_name
        .trim_start_matches(r"\_TZ.")
        .trim_start_matches(r"\\_TZ.")
        .trim_start_matches('/')
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{SensorPoller, SensorSource, publish_sensor_snapshot};
    use crate::input::{InputData, InputSource, SourceState};
    use hypercolor_types::sensor::SystemSnapshot;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[test]
    fn source_publishes_changed_snapshots_through_the_data_role() {
        let counter = Arc::new(AtomicU64::new(1));
        let mut source = SensorSource::with_interval(Duration::from_millis(20));
        let next = Arc::clone(&counter);
        source.set_test_sampler(move || SystemSnapshot {
            polled_at_ms: next.fetch_add(1, Ordering::Relaxed),
            ..SystemSnapshot::empty()
        });
        source.set_source_graph_generation(1);
        source.start().expect("sensor source should start");

        let first = wait_for_source_sample(&mut source, Duration::from_secs(1));
        let second = wait_for_source_sample(&mut source, Duration::from_secs(1));

        assert!(second > first);
        source.stop();
    }

    #[test]
    fn source_reports_worker_panics_as_terminal_failure() {
        let mut source = SensorSource::with_interval(Duration::from_millis(20));
        source.set_test_sampler(|| panic!("fixture sensor panic"));
        source.set_source_graph_generation(1);
        source.start().expect("sensor source should start");
        let deadline = Instant::now() + Duration::from_secs(1);
        let error = loop {
            match source.sample() {
                Err(error) => break error,
                Ok(_) if Instant::now() < deadline => std::thread::yield_now(),
                Ok(_) => panic!("sensor worker panic was not observed"),
            }
        };

        assert!(error.to_string().contains("panicked"));
        assert_eq!(
            source
                .source_status_handle()
                .expect("sensor source publishes status")
                .snapshot()
                .state,
            SourceState::Failed
        );
        assert!(!source.is_running());
    }

    #[test]
    fn poller_publishes_updated_snapshots() {
        let counter = Arc::new(AtomicU64::new(1));
        let mut poller = SensorPoller::with_interval(Duration::from_millis(20));
        let next = Arc::clone(&counter);
        poller.set_test_sampler(move || {
            let stamp = next.fetch_add(1, Ordering::Relaxed);
            SystemSnapshot {
                polled_at_ms: stamp,
                ..SystemSnapshot::empty()
            }
        });

        let mut rx = poller.receiver();
        poller.start().expect("poller should start");

        assert!(
            wait_for_change(&mut rx, Duration::from_secs(1)),
            "receiver should observe at least one snapshot update"
        );
        let first = rx.borrow_and_update().polled_at_ms;

        assert!(
            wait_for_change(&mut rx, Duration::from_secs(1)),
            "receiver should observe a second snapshot update"
        );
        let second = rx.borrow_and_update().polled_at_ms;

        assert!(second > first);
        poller.stop();
    }

    #[test]
    fn blocked_sampler_does_not_make_stop_unbounded() {
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let mut poller = SensorPoller::with_interval(Duration::from_millis(20));
        poller.set_test_sampler(move || {
            let _ = entered_tx.send(());
            let _ = release_rx.recv();
            SystemSnapshot::empty()
        });
        poller.start().expect("blocked sensor poller should start");
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("test sampler should enter its blocking call");

        let started = Instant::now();
        poller.stop_with_timeout(Duration::from_millis(5));

        assert!(
            started.elapsed() < Duration::from_millis(100),
            "bounded sensor stop took {:?}",
            started.elapsed()
        );
        poller.stop();
        release_tx
            .send(())
            .expect("blocked test sampler should be releasable");
    }

    #[test]
    fn timed_out_sampler_cannot_overwrite_a_successor_session() {
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let (returned_tx, returned_rx) = mpsc::sync_channel(1);
        let mut poller = SensorPoller::with_interval(Duration::from_secs(5));
        poller.set_test_sampler(move || {
            let _ = entered_tx.send(());
            let _ = release_rx.recv();
            let _ = returned_tx.send(());
            SystemSnapshot {
                polled_at_ms: 1,
                ..SystemSnapshot::empty()
            }
        });
        let mut rx = poller.receiver();
        poller.start().expect("blocked sensor poller should start");
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first session should enter its sampler");

        poller.stop_with_timeout(Duration::from_millis(5));
        assert_eq!(rx.borrow_and_update().polled_at_ms, 0);

        poller.set_test_sampler(|| SystemSnapshot {
            polled_at_ms: 2,
            ..SystemSnapshot::empty()
        });
        poller
            .start()
            .expect("successor sensor poller should start");
        assert!(
            wait_for_snapshot(&mut rx, 2, Duration::from_secs(1)),
            "successor session should publish its snapshot"
        );

        release_tx
            .send(())
            .expect("timed-out sampler should be releasable");
        returned_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("timed-out sampler should return");
        assert!(
            !wait_for_change(&mut rx, Duration::from_millis(100)),
            "retired sensor session published after its successor"
        );
        assert_eq!(rx.borrow().polled_at_ms, 2);
        poller.stop();
    }

    #[test]
    fn retired_generation_cannot_publish() {
        let (tx, rx) = tokio::sync::watch::channel(Arc::new(SystemSnapshot::empty()));
        let publication_session = Mutex::new(Some(2));
        assert!(publish_sensor_snapshot(
            &publication_session,
            2,
            &tx,
            SystemSnapshot {
                polled_at_ms: 2,
                ..SystemSnapshot::empty()
            },
        ));
        assert!(!publish_sensor_snapshot(
            &publication_session,
            1,
            &tx,
            SystemSnapshot {
                polled_at_ms: 1,
                ..SystemSnapshot::empty()
            },
        ));
        assert_eq!(rx.borrow().polled_at_ms, 2);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn wmi_access_denied_hresult_is_terminal_for_acpi_thermal_polling() {
        let denied = wmi::WMIError::HResultError {
            hres: super::WBEM_E_ACCESS_DENIED_HRESULT,
        };
        let other = wmi::WMIError::HResultError { hres: 0 };

        assert!(super::wmi_access_denied(&denied));
        assert!(!super::wmi_access_denied(&other));
    }

    fn wait_for_change(
        rx: &mut tokio::sync::watch::Receiver<Arc<SystemSnapshot>>,
        timeout: Duration,
    ) -> bool {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should build");
        runtime.block_on(async {
            matches!(
                tokio::time::timeout(timeout, rx.changed()).await,
                Ok(Ok(()))
            )
        })
    }

    fn wait_for_source_sample(source: &mut SensorSource, timeout: Duration) -> u64 {
        let deadline = Instant::now() + timeout;
        loop {
            match source
                .sample()
                .expect("sensor source sample should succeed")
            {
                InputData::Sensors(snapshot) => return snapshot.polled_at_ms,
                InputData::None if Instant::now() < deadline => std::thread::yield_now(),
                InputData::None => panic!("sensor source did not publish before the deadline"),
                sample => panic!("unexpected sensor source sample: {sample:?}"),
            }
        }
    }

    fn wait_for_snapshot(
        rx: &mut tokio::sync::watch::Receiver<Arc<SystemSnapshot>>,
        expected_polled_at_ms: u64,
        timeout: Duration,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            if !wait_for_change(rx, remaining) {
                return false;
            }
            if rx.borrow_and_update().polled_at_ms == expected_polled_at_ms {
                return true;
            }
        }
        false
    }
}
