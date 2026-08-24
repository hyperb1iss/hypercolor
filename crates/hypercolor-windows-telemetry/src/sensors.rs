//! Windows sensor cascade: PawnIO MSR/SMN, LibreHardwareMonitor /
//! OpenHardwareMonitor, and ACPI thermal zones.

use std::time::Duration;

use hypercolor_types::sensor::{SensorReading, SensorUnit, SystemSnapshot};
use tracing::debug;

/// Backoff between attempts to open the Windows PawnIO CPU temperature
/// reader while it is unavailable. Long enough that a permanently
/// PawnIO-less host pays almost nothing, short enough that installing
/// hardware support feels immediate rather than requiring a restart.
const CPU_TEMP_REPROBE_INTERVAL: Duration = Duration::from_secs(20);

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
#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
struct HardwareMonitorSensor {
    name: String,
    value: f32,
    sensor_type: String,
    identifier: String,
}

const WBEM_E_ACCESS_DENIED_HRESULT: i32 = -2_147_217_405;

/// Windows sensor extras. Builds a small cascade of sources at
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
pub struct SensorExtras {
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

impl SensorExtras {
    /// Probe every source once. Absent sources are retried from
    /// [`Self::merge_snapshot`] where that is useful (PawnIO) and otherwise
    /// stay absent for the daemon's lifetime.
    #[must_use]
    pub fn new() -> Self {
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

    /// Fold the Windows sources into `snapshot`, preferring PawnIO for the
    /// CPU package temperature and never overriding a GPU reading NVML
    /// already produced.
    pub fn merge_snapshot(&mut self, snapshot: &mut SystemSnapshot) {
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

fn wmi_access_denied(error: &wmi::WMIError) -> bool {
    match error {
        wmi::WMIError::HResultError { hres } => *hres == WBEM_E_ACCESS_DENIED_HRESULT,
        _ => false,
    }
}

/// LibreHardwareMonitor identifies CPU sensors with paths starting with
/// `/intelcpu/` or `/amdcpu/`. OpenHardwareMonitor uses the same convention.
fn is_cpu_sensor(identifier: &str, name: &str) -> bool {
    let ident = identifier.to_ascii_lowercase();
    ident.starts_with("/intelcpu/")
        || ident.starts_with("/amdcpu/")
        || ident.starts_with("/cpu/")
        || name.to_ascii_lowercase().starts_with("cpu ")
        || name.eq_ignore_ascii_case("cpu")
}

fn is_cpu_package(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("package")
        || lower.contains("tdie")
        || lower.contains("tctl")
        || lower.contains("ccd average")
        || lower == "cpu total"
}

fn is_gpu_sensor(identifier: &str, name: &str) -> bool {
    let ident = identifier.to_ascii_lowercase();
    ident.starts_with("/nvidiagpu/")
        || ident.starts_with("/atigpu/")
        || ident.starts_with("/amdgpu/")
        || ident.starts_with("/gpu/")
        || name.to_ascii_lowercase().starts_with("gpu ")
}

fn is_gpu_core(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("core") || lower.contains("die") || lower == "gpu"
}

/// ACPI thermal zones report `CurrentTemperature` in tenths of a Kelvin per
/// `MSAcpi_ThermalZoneTemperature` documentation. Convert to Celsius.
#[allow(clippy::as_conversions, clippy::cast_precision_loss)]
fn deci_kelvin_to_celsius(value: u32) -> f32 {
    (f64::from(value) / 10.0 - 273.15) as f32
}

/// Strip ACPI / HardwareMonitor path prefixes for clean labels.
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

impl Default for SensorExtras {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn wmi_access_denied_hresult_is_terminal_for_acpi_thermal_polling() {
        let denied = wmi::WMIError::HResultError {
            hres: super::WBEM_E_ACCESS_DENIED_HRESULT,
        };
        let other = wmi::WMIError::HResultError { hres: 0 };

        assert!(super::wmi_access_denied(&denied));
        assert!(!super::wmi_access_denied(&other));
    }
}
