use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use arc_swap::ArcSwap;
use hypercolor_core::input::{
    InputGraphHandle, InputGraphSnapshot, InputManager, SourceKind, SourceState,
};
use hypercolor_types::sensor::SystemSnapshot;
use tokio::sync::{Mutex, Notify, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant as TokioInstant, timeout};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use super::capture_demand::CaptureDemandState;

const STOP_TIMEOUT: Duration = Duration::from_secs(1);
const LIFECYCLE_PROBE_INTERVAL: Duration = Duration::from_millis(250);
const SOURCE_KINDS: [SourceKind; 5] = [
    SourceKind::Audio,
    SourceKind::Screen,
    SourceKind::Interaction,
    SourceKind::Media,
    SourceKind::Network,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// A consumer class contributing input-publication cadence demand.
pub enum InputPublicationConsumer {
    /// The hardware-authoritative scene renderer.
    Authoritative,
    /// An isolated interactive preview renderer.
    Preview,
    /// A latest-value stream that does not render hardware output.
    PassiveStream,
    /// A diagnostic reader that explicitly requests live samples.
    Diagnostic,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// Per-source publication rates requested by one consumer class.
pub struct InputPublicationDemand {
    audio: u32,
    screen: u32,
    interaction: u32,
    media: u32,
    network: u32,
}

impl InputPublicationDemand {
    /// Request the same rate for every typed source.
    #[must_use]
    pub const fn all_sources(requested_hz: u32) -> Self {
        Self {
            audio: requested_hz,
            screen: requested_hz,
            interaction: requested_hz,
            media: requested_hz,
            network: requested_hz,
        }
    }

    /// Set one typed source rate, preserving the other source rates.
    #[must_use]
    pub const fn with_source(mut self, source: SourceKind, requested_hz: u32) -> Self {
        match source {
            SourceKind::Audio => self.audio = requested_hz,
            SourceKind::Screen => self.screen = requested_hz,
            SourceKind::Interaction => self.interaction = requested_hz,
            SourceKind::Media => self.media = requested_hz,
            SourceKind::Network => self.network = requested_hz,
        }
        self
    }

    pub(crate) const fn requested_hz(self, source: SourceKind) -> u32 {
        match source {
            SourceKind::Audio => self.audio,
            SourceKind::Screen => self.screen,
            SourceKind::Interaction => self.interaction,
            SourceKind::Media => self.media,
            SourceKind::Network => self.network,
        }
    }

    const fn max_requested_hz(self) -> u32 {
        let max = if self.audio > self.screen {
            self.audio
        } else {
            self.screen
        };
        let max = if max > self.interaction {
            max
        } else {
            self.interaction
        };
        let max = if max > self.media { max } else { self.media };
        if max > self.network {
            max
        } else {
            self.network
        }
    }

    const fn union(self, other: Self) -> Self {
        Self {
            audio: if self.audio > other.audio {
                self.audio
            } else {
                other.audio
            },
            screen: if self.screen > other.screen {
                self.screen
            } else {
                other.screen
            },
            interaction: if self.interaction > other.interaction {
                self.interaction
            } else {
                other.interaction
            },
            media: if self.media > other.media {
                self.media
            } else {
                other.media
            },
            network: if self.network > other.network {
                self.network
            } else {
                other.network
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct InputPublicationDemandEntry {
    id: u64,
    consumer: InputPublicationConsumer,
    demand: InputPublicationDemand,
}

#[derive(Clone, Debug, Default)]
struct InputPublicationDemandSnapshot {
    entries: Arc<[InputPublicationDemandEntry]>,
    aggregate: InputPublicationDemand,
}

impl InputPublicationDemandSnapshot {
    fn from_entries(entries: Vec<InputPublicationDemandEntry>) -> Self {
        let aggregate = entries
            .iter()
            .fold(InputPublicationDemand::default(), |aggregate, entry| {
                aggregate.union(entry.demand)
            });
        Self {
            entries: entries.into(),
            aggregate,
        }
    }

    fn requested_hz(&self, source: SourceKind) -> u32 {
        self.aggregate.requested_hz(source)
    }

    fn max_requested_hz(&self) -> u32 {
        self.aggregate.max_requested_hz()
    }

    fn registration_count(&self, consumer: InputPublicationConsumer) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.consumer == consumer)
            .count()
    }
}

struct InputPublicationDemandRegistry {
    next_id: AtomicU64,
    latest: ArcSwap<InputPublicationDemandSnapshot>,
    changed: Notify,
}

#[derive(Clone)]
/// Lock-free latest-value demand publication for all input consumers.
pub struct InputPublicationDemandHandle {
    registry: Arc<InputPublicationDemandRegistry>,
}

impl InputPublicationDemandHandle {
    /// Create an empty demand publication.
    #[must_use]
    pub fn new() -> Self {
        Self {
            registry: Arc::new(InputPublicationDemandRegistry {
                next_id: AtomicU64::new(1),
                latest: ArcSwap::from_pointee(InputPublicationDemandSnapshot::default()),
                changed: Notify::new(),
            }),
        }
    }

    /// Register one independently owned demand contribution.
    #[must_use = "dropping the registration immediately removes its demand"]
    pub fn register(
        &self,
        consumer: InputPublicationConsumer,
        demand: InputPublicationDemand,
    ) -> InputPublicationDemandRegistration {
        let id = self
            .registry
            .next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next_id| {
                next_id.checked_add(1)
            })
            .expect("input publication demand registration identity exhausted");
        self.registry.update_entries(|entries| {
            entries.push(InputPublicationDemandEntry {
                id,
                consumer,
                demand,
            });
        });
        InputPublicationDemandRegistration {
            registry: Arc::clone(&self.registry),
            id,
        }
    }

    /// Count live registrations owned by one consumer class.
    #[must_use]
    pub fn registration_count(&self, consumer: InputPublicationConsumer) -> usize {
        self.snapshot().registration_count(consumer)
    }

    /// Read the current aggregate rate for one source domain.
    #[must_use]
    pub fn requested_hz(&self, source: SourceKind) -> u32 {
        self.snapshot().requested_hz(source)
    }

    pub(crate) fn is_active(&self) -> bool {
        self.snapshot().max_requested_hz() > 0
    }

    fn snapshot(&self) -> Arc<InputPublicationDemandSnapshot> {
        self.registry.latest.load_full()
    }

    async fn changed(&self) {
        self.registry.changed.notified().await;
    }
}

impl Default for InputPublicationDemandHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl InputPublicationDemandRegistry {
    fn update_entries(&self, update: impl Fn(&mut Vec<InputPublicationDemandEntry>)) {
        self.latest.rcu(|current| {
            let mut entries = current.entries.to_vec();
            update(&mut entries);
            Arc::new(InputPublicationDemandSnapshot::from_entries(entries))
        });
        self.changed.notify_one();
    }

    fn update_registration(&self, id: u64, demand: InputPublicationDemand) {
        self.update_entries(|entries| {
            if let Some(entry) = entries.iter_mut().find(|entry| entry.id == id) {
                entry.demand = demand;
            }
        });
    }

    fn remove_registration(&self, id: u64) {
        self.update_entries(|entries| entries.retain(|entry| entry.id != id));
    }
}

/// RAII ownership of one input-publication demand contribution.
pub struct InputPublicationDemandRegistration {
    registry: Arc<InputPublicationDemandRegistry>,
    id: u64,
}

impl InputPublicationDemandRegistration {
    /// Replace this registration's typed demand atomically.
    pub fn update(&self, demand: InputPublicationDemand) {
        self.registry.update_registration(self.id, demand);
    }
}

impl Drop for InputPublicationDemandRegistration {
    fn drop(&mut self) {
        self.registry.remove_registration(self.id);
    }
}

pub(crate) struct OwnedInputPublicationDemand {
    registration: InputPublicationDemandRegistration,
    current: InputPublicationDemand,
}

impl OwnedInputPublicationDemand {
    pub(crate) fn new(
        demands: &InputPublicationDemandHandle,
        consumer: InputPublicationConsumer,
    ) -> Self {
        Self {
            registration: demands.register(consumer, InputPublicationDemand::default()),
            current: InputPublicationDemand::default(),
        }
    }

    pub(crate) fn publish(&mut self, demand: InputPublicationDemand) {
        if demand != self.current {
            self.registration.update(demand);
            self.current = demand;
        }
    }

    pub(crate) fn clear(&mut self) {
        self.publish(InputPublicationDemand::default());
    }
}

#[derive(Clone)]
pub(crate) struct InputPublicationReader {
    graph: InputGraphHandle,
    sensors: Option<watch::Receiver<Arc<SystemSnapshot>>>,
}

impl InputPublicationReader {
    fn new(graph: InputGraphHandle, sensors: Option<watch::Receiver<Arc<SystemSnapshot>>>) -> Self {
        Self { graph, sensors }
    }

    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self::new(InputGraphHandle::default(), None)
    }

    pub(crate) fn graph_snapshot(&self) -> Arc<InputGraphSnapshot> {
        self.graph.snapshot()
    }

    pub(crate) fn latest_sensor_snapshot(&self) -> Option<Arc<SystemSnapshot>> {
        self.sensors
            .as_ref()
            .map(|receiver| Arc::clone(&receiver.borrow()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Observable lifecycle state of the dedicated input-publication pump.
pub enum InputPublicationStatus {
    /// The worker has been spawned but has not published readiness.
    Starting,
    /// The worker is ready to service source demand.
    Ready,
    /// The worker completed an intentional shutdown.
    Stopped,
    /// The worker exited unexpectedly.
    Failed(Arc<str>),
}

impl InputPublicationStatus {
    const fn is_terminal(&self) -> bool {
        matches!(self, Self::Stopped | Self::Failed(_))
    }
}

#[derive(Clone)]
pub(crate) struct InputPublicationMonitor {
    status: watch::Receiver<InputPublicationStatus>,
}

impl InputPublicationMonitor {
    pub(crate) fn status(&self) -> InputPublicationStatus {
        self.status.borrow().clone()
    }

    pub(crate) async fn wait_for_terminal(mut self) -> InputPublicationStatus {
        loop {
            let status = self.status();
            if status.is_terminal() {
                return status;
            }
            if self.status.changed().await.is_err() {
                return InputPublicationStatus::Failed(Arc::from(
                    "input publication status channel closed",
                ));
            }
        }
    }
}

pub(crate) struct InputPublicationPump {
    cancel: CancellationToken,
    supervisor: Option<JoinHandle<Result<()>>>,
    reader: InputPublicationReader,
    monitor: InputPublicationMonitor,
}

impl InputPublicationPump {
    pub(crate) async fn start(
        manager: Arc<Mutex<InputManager>>,
        demands: InputPublicationDemandHandle,
    ) -> Result<Self> {
        let reader = {
            let manager = manager.lock().await;
            InputPublicationReader::new(
                manager.input_graph_handle(),
                manager.sensor_snapshot_receiver(),
            )
        };
        let cancel = CancellationToken::new();
        let (ready_tx, ready_rx) = oneshot::channel();
        let (status_tx, status_rx) = watch::channel(InputPublicationStatus::Starting);
        let worker_cancel = cancel.clone();
        let worker_reader = reader.clone();
        let supervisor = tokio::spawn(async move {
            let worker_status = status_tx.clone();
            let worker = tokio::spawn(run_pump(
                manager,
                worker_reader,
                demands,
                worker_cancel,
                worker_status,
                ready_tx,
            ));
            let worker = AbortOnDropTask::new(worker);
            match worker.join().await {
                Ok(()) => {
                    status_tx.send_replace(InputPublicationStatus::Stopped);
                    Ok(())
                }
                Err(error) => {
                    let message: Arc<str> =
                        Arc::from(format!("input publication worker terminated: {error}"));
                    status_tx.send_replace(InputPublicationStatus::Failed(Arc::clone(&message)));
                    Err(anyhow!(message.to_string()))
                }
            }
        });
        let monitor = InputPublicationMonitor { status: status_rx };

        if ready_rx.await.is_err() {
            supervisor.abort();
            let _ = supervisor.await;
            return Err(match monitor.status() {
                InputPublicationStatus::Failed(message) => anyhow!(message.to_string()),
                status => anyhow!("input publication pump stopped during startup: {status:?}"),
            });
        }
        if monitor.status() != InputPublicationStatus::Ready {
            supervisor.abort();
            let _ = supervisor.await;
            return Err(anyhow!(
                "input publication pump did not reach readiness: {:?}",
                monitor.status()
            ));
        }
        info!("input publication pump started");

        Ok(Self {
            cancel,
            supervisor: Some(supervisor),
            reader,
            monitor,
        })
    }

    pub(crate) fn reader(&self) -> InputPublicationReader {
        self.reader.clone()
    }

    pub(crate) fn monitor(&self) -> InputPublicationMonitor {
        self.monitor.clone()
    }

    pub(crate) async fn shutdown(&mut self) -> Result<()> {
        self.cancel.cancel();
        let Some(mut supervisor) = self.supervisor.take() else {
            return Ok(());
        };

        if let Ok(joined) = timeout(STOP_TIMEOUT, &mut supervisor).await {
            joined.context("input publication supervisor task panicked")??;
        } else {
            supervisor.abort();
            let _ = timeout(STOP_TIMEOUT, &mut supervisor).await;
            return Err(anyhow!(
                "input publication pump exceeded its bounded shutdown deadline"
            ));
        }
        info!("input publication pump stopped");
        Ok(())
    }
}

impl Drop for InputPublicationPump {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(supervisor) = &self.supervisor {
            supervisor.abort();
        }
    }
}

struct AbortOnDropTask<T> {
    handle: Option<JoinHandle<T>>,
}

impl<T> AbortOnDropTask<T> {
    const fn new(handle: JoinHandle<T>) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    async fn join(mut self) -> std::result::Result<T, tokio::task::JoinError> {
        let result = self
            .handle
            .as_mut()
            .expect("abort-on-drop task retains its join handle")
            .await;
        self.handle = None;
        result
    }
}

impl<T> Drop for AbortOnDropTask<T> {
    fn drop(&mut self) {
        if let Some(handle) = &self.handle {
            handle.abort();
        }
    }
}

async fn run_pump(
    manager: Arc<Mutex<InputManager>>,
    reader: InputPublicationReader,
    demands: InputPublicationDemandHandle,
    cancel: CancellationToken,
    status: watch::Sender<InputPublicationStatus>,
    ready: oneshot::Sender<()>,
) {
    status.send_replace(InputPublicationStatus::Ready);
    let _ = ready.send(());

    let mut schedule = InputPublicationSchedule::default();
    let mut capture_demand = CaptureDemandState::default();
    let mut due_sources = Vec::with_capacity(SOURCE_KINDS.len());
    let mut graph_changes = reader.graph.subscribe_generation();
    loop {
        let demand = demands.snapshot();
        let mut graph = reader.graph_snapshot();
        if !capture_demand.is_current(graph.generation(), demand.aggregate) {
            let manager_lock = manager.lock();
            tokio::pin!(manager_lock);
            let mut input_manager = tokio::select! {
                () = cancel.cancelled() => break,
                () = demands.changed() => continue,
                manager = &mut manager_lock => manager,
            };
            capture_demand.reconcile(&mut input_manager, demand.aggregate);
            drop(input_manager);
            graph = reader.graph_snapshot();
        }
        let lifecycle_current = capture_demand.is_current(graph.generation(), demand.aggregate);
        let active_demand = demand_for_active_sources(&graph, &demand);
        let now = Instant::now();
        schedule.synchronize(active_demand, now);

        if demand.max_requested_hz() == 0 && lifecycle_current {
            tokio::select! {
                () = cancel.cancelled() => break,
                () = demands.changed() => {}
                _ = graph_changes.changed() => {}
            }
            continue;
        }

        if demand.max_requested_hz() == 0 {
            tokio::select! {
                () = cancel.cancelled() => break,
                () = demands.changed() => {}
                _ = graph_changes.changed() => {}
                () = tokio::time::sleep(LIFECYCLE_PROBE_INTERVAL) => {}
            }
            continue;
        }

        if active_demand.max_requested_hz() == 0 {
            tokio::select! {
                () = cancel.cancelled() => break,
                () = demands.changed() => {}
                _ = graph_changes.changed() => {}
                () = tokio::time::sleep(LIFECYCLE_PROBE_INTERVAL) => {}
            }
            continue;
        }

        if !schedule.is_due(now) {
            let lifecycle_probe = now.checked_add(LIFECYCLE_PROBE_INTERVAL).unwrap_or(now);
            let wake_at = schedule
                .next_deadline()
                .map_or(lifecycle_probe, |deadline| deadline.min(lifecycle_probe));
            tokio::select! {
                () = cancel.cancelled() => break,
                () = demands.changed() => {}
                _ = graph_changes.changed() => {}
                () = tokio::time::sleep_until(TokioInstant::from_std(wake_at)) => {}
            }
            continue;
        }

        let manager_lock = manager.lock();
        tokio::pin!(manager_lock);
        let mut manager = tokio::select! {
            () = cancel.cancelled() => break,
            () = demands.changed() => continue,
            _ = graph_changes.changed() => continue,
            manager = &mut manager_lock => manager,
        };
        schedule.collect_due(Instant::now(), &mut due_sources);
        manager.sample_source_kinds(&due_sources);
    }
    debug!("input publication worker exited");
}

fn demand_for_active_sources(
    graph: &InputGraphSnapshot,
    demand: &InputPublicationDemandSnapshot,
) -> InputPublicationDemand {
    let now = Instant::now();
    graph
        .slots()
        .iter()
        .fold(InputPublicationDemand::default(), |active_demand, slot| {
            let status = slot.status().availability_at(now);
            if status.retired
                || !(status.configured && status.consented && status.demanded)
                || !matches!(
                    status.state,
                    SourceState::Starting | SourceState::Live | SourceState::Degraded
                )
            {
                return active_demand;
            }
            active_demand.with_source(status.kind, demand.requested_hz(status.kind))
        })
}

#[derive(Clone, Copy, Debug, Default)]
struct SourceCadence {
    requested_hz: u32,
    last_sample_at: Option<Instant>,
    next_sample_at: Option<Instant>,
}

#[derive(Debug, Default)]
struct InputPublicationSchedule {
    sources: [SourceCadence; SOURCE_KINDS.len()],
}

impl InputPublicationSchedule {
    fn synchronize(&mut self, demand: InputPublicationDemand, now: Instant) {
        for source in SOURCE_KINDS {
            let cadence = &mut self.sources[source_kind_index(source)];
            let requested_hz = demand.requested_hz(source);
            if requested_hz == 0 {
                *cadence = SourceCadence::default();
            } else if cadence.requested_hz != requested_hz {
                cadence.requested_hz = requested_hz;
                cadence.next_sample_at =
                    Some(cadence.last_sample_at.map_or(now, |last_sample_at| {
                        let deadline = last_sample_at
                            .checked_add(cadence_interval(requested_hz))
                            .unwrap_or(now);
                        deadline.max(now)
                    }));
            }
        }
    }

    fn collect_due(&mut self, now: Instant, output: &mut Vec<(SourceKind, f32)>) {
        output.clear();
        for source in SOURCE_KINDS {
            let cadence = &mut self.sources[source_kind_index(source)];
            let Some(next_sample_at) = cadence.next_sample_at else {
                continue;
            };
            if next_sample_at > now {
                continue;
            }
            let interval = cadence_interval(cadence.requested_hz);
            let delta_secs = cadence
                .last_sample_at
                .map_or(interval.as_secs_f32(), |previous| {
                    now.saturating_duration_since(previous).as_secs_f32()
                });
            output.push((source, delta_secs));
            cadence.last_sample_at = Some(now);
            cadence.next_sample_at = Some(next_deadline(next_sample_at, interval, now));
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.sources
            .iter()
            .filter_map(|cadence| cadence.next_sample_at)
            .min()
    }

    fn is_due(&self, now: Instant) -> bool {
        self.next_deadline().is_some_and(|deadline| deadline <= now)
    }
}

const fn source_kind_index(source: SourceKind) -> usize {
    match source {
        SourceKind::Audio => 0,
        SourceKind::Screen => 1,
        SourceKind::Interaction => 2,
        SourceKind::Media => 3,
        SourceKind::Network => 4,
    }
}

fn cadence_interval(requested_hz: u32) -> Duration {
    let nanos = 1_000_000_000_u64.div_ceil(u64::from(requested_hz));
    Duration::from_nanos(nanos)
}

fn next_deadline(scheduled: Instant, interval: Duration, now: Instant) -> Instant {
    let next = scheduled.checked_add(interval).unwrap_or(now);
    if next > now {
        next
    } else {
        now.checked_add(interval).unwrap_or(now)
    }
}

#[cfg(test)]
mod tests;
