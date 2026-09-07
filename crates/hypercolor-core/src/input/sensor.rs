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
    windows: hypercolor_windows_telemetry::SensorExtras,
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
            windows: hypercolor_windows_telemetry::SensorExtras::new(),
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
