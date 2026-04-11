//! Background system sensor polling for the render pipeline.

use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use tokio::sync::watch;
use tracing::debug;

use hypercolor_types::sensor::{SensorReading, SensorUnit, SystemSnapshot};
use sysinfo::{Components, MINIMUM_CPU_UPDATE_INTERVAL, System};

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
    #[cfg(feature = "nvidia")]
    nvidia: Option<NvidiaTelemetry>,
}

impl SystemSampler {
    fn new() -> Self {
        let mut system = System::new_all();
        system.refresh_memory();
        system.refresh_cpu_usage();

        let mut components = Components::new_with_refreshed_list();
        components.refresh(false);

        Self {
            system,
            components,
            #[cfg(feature = "nvidia")]
            nvidia: NvidiaTelemetry::new(),
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

        let snapshot = SystemSnapshot {
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

        #[cfg(feature = "nvidia")]
        if let Some(nvidia) = self.nvidia.as_mut() {
            nvidia.merge_snapshot(&mut snapshot);
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

#[cfg(feature = "nvidia")]
struct NvidiaTelemetry {
    nvml: nvml_wrapper::Nvml,
}

#[cfg(feature = "nvidia")]
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
