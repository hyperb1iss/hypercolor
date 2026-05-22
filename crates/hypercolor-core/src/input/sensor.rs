//! Background system sensor polling for the render pipeline.

use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use tokio::sync::watch;
use tracing::debug;

use hypercolor_types::sensor::{SensorReading, SensorUnit, SystemSnapshot};
use sysinfo::{
    Components, CpuRefreshKind, MINIMUM_CPU_UPDATE_INTERVAL, MemoryRefreshKind, RefreshKind, System,
};

const DEFAULT_SENSOR_POLL_INTERVAL: Duration = Duration::from_secs(2);
const BYTES_PER_MEGABYTE: f64 = 1_000_000.0;

/// Background poller that publishes latest-value system telemetry snapshots.
pub struct SensorPoller {
    interval: Duration,
    tx: watch::Sender<Arc<SystemSnapshot>>,
    thread: Option<SensorPollerThread>,
    #[cfg(test)]
    sampler: Option<Box<dyn FnMut() -> SystemSnapshot + Send>>,
}

struct SensorPollerThread {
    stop_tx: Sender<()>,
    join_handle: JoinHandle<()>,
}

impl SensorPoller {
    /// Create a new poller using the default cadence.
    #[must_use]
    pub fn new() -> Self {
        Self::with_interval(DEFAULT_SENSOR_POLL_INTERVAL)
    }

    /// Create a new poller with a custom cadence.
    #[must_use]
    pub fn with_interval(interval: Duration) -> Self {
        let (tx, _) = watch::channel(Arc::new(SystemSnapshot::empty()));
        Self {
            interval: interval.max(MINIMUM_CPU_UPDATE_INTERVAL),
            tx,
            thread: None,
            #[cfg(test)]
            sampler: None,
        }
    }

    /// Subscribe to latest-value snapshots.
    #[must_use]
    pub fn receiver(&self) -> watch::Receiver<Arc<SystemSnapshot>> {
        self.tx.subscribe()
    }

    /// Start the poller thread if it is not already running.
    ///
    /// # Errors
    ///
    /// Returns an error if the poller thread cannot be spawned.
    pub fn start(&mut self) -> Result<()> {
        if self.thread.is_some() {
            return Ok(());
        }

        let interval = self.interval;
        let tx = self.tx.clone();
        let (stop_tx, stop_rx) = mpsc::channel();
        #[cfg(test)]
        let mut sampler = self.sampler.take();
        let join_handle = std::thread::Builder::new()
            .name("hypercolor-sensors".to_owned())
            .spawn(move || {
                #[cfg(test)]
                if let Some(ref mut sampler) = sampler {
                    loop {
                        tx.send_replace(Arc::new(sampler()));
                        match stop_rx.recv_timeout(interval) {
                            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                            Err(RecvTimeoutError::Timeout) => {}
                        }
                    }
                    return;
                }

                let mut sampler = SystemSampler::new();
                loop {
                    tx.send_replace(Arc::new(sampler.sample_snapshot()));
                    match stop_rx.recv_timeout(interval) {
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                }
            })
            .context("failed to spawn sensor poller thread")?;

        self.thread = Some(SensorPollerThread {
            stop_tx,
            join_handle,
        });
        Ok(())
    }

    /// Stop the poller thread if it is running.
    pub fn stop(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };

        let _ = thread.stop_tx.send(());
        if let Err(error) = thread.join_handle.join() {
            debug!("sensor poller thread join failed: {error:?}");
        }
    }

    #[cfg(test)]
    pub(crate) fn set_test_sampler(
        &mut self,
        sampler: impl FnMut() -> SystemSnapshot + Send + 'static,
    ) {
        self.sampler = Some(Box::new(sampler));
    }
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
    windows: Option<WindowsSensorExtras>,
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
        if let Some(windows) = self.windows.as_mut() {
            windows.merge_snapshot(&mut snapshot);
        }

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

/// Windows-specific sensor extras. Builds a small cascade of WMI sources at
/// startup and queries the available ones every poll:
///
/// 1. **LibreHardwareMonitor** (`ROOT\LibreHardwareMonitor.Sensor`) — most
///    accurate, gives per-core CPU + GPU + VRM data when the tool is running.
/// 2. **OpenHardwareMonitor** (`ROOT\OpenHardwareMonitor.Sensor`) — same
///    schema; older but still common on enthusiast rigs.
/// 3. **MSAcpi_ThermalZoneTemperature** (`ROOT\WMI`) — ubiquitous on systems
///    with ACPI thermal zones but sparse on modern boards (often absent or
///    reporting a single chassis-level value).
///
/// On a fresh consumer install with none of LHM/OHM running and no ACPI
/// thermal zones exposed, we won't return CPU temps until the user installs
/// LibreHardwareMonitor (recommended) or PawnIO is wired for MSR reads
/// (later phase).
#[cfg(target_os = "windows")]
struct WindowsSensorExtras {
    libre_hardware: Option<wmi::WMIConnection>,
    open_hardware: Option<wmi::WMIConnection>,
    acpi_zones: Option<wmi::WMIConnection>,
    acpi_zones_enabled: bool,
}

#[cfg(target_os = "windows")]
impl WindowsSensorExtras {
    fn new() -> Option<Self> {
        // Opportunistic sources — used if the user happens to be running
        // LibreHardwareMonitor or OpenHardwareMonitor, but never required.
        // The first-class CPU temp path is PawnIO MSR reads (TODO: wire),
        // which we bundle with the installer for motherboard RGB anyway.
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

        if libre_hardware.is_none() && open_hardware.is_none() && acpi_zones.is_none() {
            return None;
        }

        Some(Self {
            libre_hardware,
            open_hardware,
            acpi_zones,
            acpi_zones_enabled: true,
        })
    }

    fn merge_snapshot(&mut self, snapshot: &mut SystemSnapshot) {
        // 1. LibreHardwareMonitor / OpenHardwareMonitor — only one runs at a time.
        if let Some(con) = self.libre_hardware.as_ref() {
            self.merge_hardware_monitor(con, snapshot, "lhm");
        } else if let Some(con) = self.open_hardware.as_ref() {
            self.merge_hardware_monitor(con, snapshot, "ohm");
        }

        // 2. ACPI thermal zones — additive context, only backfills cpu_temp
        //    if no better source produced one.
        if self.acpi_zones_enabled {
            if let Some(con) = self.acpi_zones.as_ref() {
                let still_enabled = merge_acpi_thermal_zones(con, snapshot);
                self.acpi_zones_enabled = still_enabled;
            }
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
                            best_cpu_temp = Some(value);
                        } else if best_cpu_temp.is_none() {
                            best_cpu_temp = Some(value);
                        } else {
                            best_cpu_temp = best_cpu_temp.map(|cur| cur.max(value));
                        }
                    } else if is_gpu_sensor(&sensor.identifier, &sensor.name) {
                        if is_gpu_core(&sensor.name) {
                            best_gpu_temp = Some(value);
                        } else if best_gpu_temp.is_none() {
                            best_gpu_temp = Some(value);
                        }
                    }
                    snapshot.components.push(SensorReading::new(
                        format!(
                            "{label_prefix}_{}",
                            sanitize_zone_label(&sensor.identifier)
                        ),
                        value,
                        SensorUnit::Celsius,
                        None,
                        None,
                        None,
                    ));
                }
                "Load" => {
                    if is_gpu_sensor(&sensor.identifier, &sensor.name) && is_gpu_core(&sensor.name)
                    {
                        best_gpu_load = Some(value);
                    }
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
fn merge_acpi_thermal_zones(
    con: &wmi::WMIConnection,
    snapshot: &mut SystemSnapshot,
) -> bool {
    let zones: Vec<MSAcpi_ThermalZoneTemperature> = match con.query() {
        Ok(zones) => zones,
        Err(err) => {
            if wmi_access_denied(&err) {
                debug!(
                    "ACPI thermal zone query denied; disabling for this session"
                );
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
    (value as f64 / 10.0 - 273.15) as f32
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
    use super::SensorPoller;
    use hypercolor_types::sensor::SystemSnapshot;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

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
}
