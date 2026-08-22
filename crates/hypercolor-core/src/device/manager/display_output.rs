use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use hypercolor_types::device::{DeviceError, DeviceId, OwnedDisplayFramePayload};
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::device::output_queue::{AsyncWriteFailureTracker, next_queue_generation};
use crate::device::traits::{
    DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryObserver, DeviceDeliveryStatus,
    DeviceDisplaySink,
};

use super::{AsyncWriteFailure, BackendHandle, BackendManager};

/// Cloneable display transport lane owned by the device output coordinator.
#[derive(Clone)]
pub struct DisplayOutputLane(Arc<DisplayOutputLaneInner>);

/// Telemetry for one coordinator-owned display output lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayOutputStatistics {
    /// Generation qualifying this lane's delivery counters.
    pub queue_generation: u64,
    /// Total display payloads that began transport I/O.
    pub transport_started: u64,
    /// Total display payloads completed by the transport.
    pub transport_completed: u64,
    /// Total display payloads rejected or failed by the transport.
    pub transport_failed: u64,
}

/// Coordinator-owned display delivery supervision snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayDeliverySupervisorStatistics {
    /// Delivery generations retained for active lanes or terminal failures.
    pub retained_generations: usize,
    /// Physical deliveries whose terminal acknowledgement is still pending.
    pub in_flight: usize,
}

const DISPLAY_SINK_LOOKUP_RETRY_INTERVAL: Duration = Duration::from_millis(250);

struct DisplayLaneState {
    display_sink: Option<Arc<dyn DeviceDisplaySink>>,
    next_display_sink_lookup_at: Option<Instant>,
}

struct DisplayOutputLaneInner {
    backend: BackendHandle,
    device_id: DeviceId,
    state: StdMutex<DisplayLaneState>,
    delivery_gate: Arc<Mutex<()>>,
    backend_generation: u64,
    queue_generation: u64,
    active: AtomicBool,
    failure_tracker: Arc<AsyncWriteFailureTracker>,
    delivery_authority: Arc<DisplayDeliveryAuthority>,
    transport_started: AtomicU64,
    transport_completed: AtomicU64,
    transport_failed: AtomicU64,
}

#[derive(Default)]
pub(super) struct DisplayDeliveryAuthority {
    state: StdMutex<DisplayDeliveryAuthorityState>,
}

#[derive(Default)]
struct DisplayDeliveryAuthorityState {
    generations: HashMap<u64, DisplayDeliveryGeneration>,
}

struct DisplayDeliveryGeneration {
    tracker: Arc<AsyncWriteFailureTracker>,
    accepting: bool,
    in_flight: HashMap<DeviceDeliveryId, BackendHandle>,
}

struct DisplayDeliveryCompletion {
    lane: Weak<DisplayOutputLaneInner>,
    device_id: DeviceId,
    failure_tracker: Arc<AsyncWriteFailureTracker>,
    delivery_authority: Weak<DisplayDeliveryAuthority>,
    delivery_id: DeviceDeliveryId,
    delivery_guard: StdMutex<Option<OwnedMutexGuard<()>>>,
    transport_started: AtomicBool,
    terminal: AtomicBool,
}

impl DisplayOutputLane {
    pub(super) fn new(
        backend_id: String,
        backend: BackendHandle,
        device_id: DeviceId,
        delivery_gate: Arc<Mutex<()>>,
        backend_generation: u64,
        delivery_authority: Arc<DisplayDeliveryAuthority>,
    ) -> Self {
        let queue_generation = next_queue_generation();
        let failure_tracker =
            delivery_authority.register_generation(backend_id, device_id, queue_generation);
        let display_sink = backend.display_sink(&device_id);
        Self(Arc::new(DisplayOutputLaneInner {
            backend,
            device_id,
            state: StdMutex::new(DisplayLaneState {
                display_sink,
                next_display_sink_lookup_at: None,
            }),
            delivery_gate,
            backend_generation,
            queue_generation,
            active: AtomicBool::new(true),
            failure_tracker,
            delivery_authority,
            transport_started: AtomicU64::new(0),
            transport_completed: AtomicU64::new(0),
            transport_failed: AtomicU64::new(0),
        }))
    }

    /// Queue generation shared with LED output telemetry identities.
    #[must_use]
    pub fn queue_generation(&self) -> u64 {
        self.0.queue_generation
    }

    /// Backend registration generation served by this lane.
    #[must_use]
    pub fn backend_generation(&self) -> u64 {
        self.0.backend_generation
    }

    /// Whether the coordinator still owns this lane generation.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.0.active.load(Ordering::Acquire)
    }

    /// Return the current display delivery telemetry snapshot.
    #[must_use]
    pub fn statistics(&self) -> DisplayOutputStatistics {
        DisplayOutputStatistics {
            queue_generation: self.0.queue_generation,
            transport_started: self.0.transport_started.load(Ordering::Relaxed),
            transport_completed: self.0.transport_completed.load(Ordering::Relaxed),
            transport_failed: self.0.transport_failed.load(Ordering::Relaxed),
        }
    }

    /// Deliver an owned display payload through the current per-device sink.
    ///
    /// A failed sink is evicted so the next delivery can adopt a replacement
    /// without stale transport state outside the lane. The backend path is used
    /// only while no per-device sink is available. The terminal transaction
    /// outlives a cancelled caller so generation ownership and failure
    /// attribution remain tied to physical transport completion.
    ///
    /// # Errors
    ///
    /// Returns the typed transport error from the selected output path.
    pub async fn write(&self, payload: Arc<OwnedDisplayFramePayload>) -> Result<(), DeviceError> {
        let delivery_guard = Arc::clone(&self.0.delivery_gate).lock_owned().await;
        let Some(delivery_id) = self
            .0
            .delivery_authority
            .begin_delivery(self.0.queue_generation, Arc::clone(&self.0.backend))
        else {
            return Err(DeviceError::Disconnected {
                device: self.0.device_id.to_string(),
            });
        };
        let completion = Arc::new(DisplayDeliveryCompletion::new(
            Arc::clone(&self.0),
            delivery_id,
            delivery_guard,
        ));
        let now = Instant::now();
        let (cached_sink, lookup_due) = {
            let state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
            (
                state.display_sink.clone(),
                state.display_sink.is_none()
                    && state
                        .next_display_sink_lookup_at
                        .is_none_or(|retry_at| now >= retry_at),
            )
        };
        let sink = if cached_sink.is_some() || !lookup_due {
            cached_sink
        } else {
            let sink = self.0.backend.display_sink(&self.0.device_id);
            let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.display_sink.clone_from(&sink);
            state.next_display_sink_lookup_at = sink
                .is_none()
                .then_some(now + DISPLAY_SINK_LOOKUP_RETRY_INTERVAL);
            sink
        };

        let observer: Arc<dyn DeviceDeliveryObserver> = completion.clone();
        let ack = match sink {
            Some(sink) => {
                sink.deliver_display_payload_owned_observed(
                    delivery_id,
                    Arc::clone(&payload),
                    observer,
                )
                .await
            }
            None => {
                self.0
                    .backend
                    .deliver_display_payload_owned_observed(
                        &self.0.device_id,
                        delivery_id,
                        payload,
                        observer,
                    )
                    .await
            }
        };
        completion.finish(&ack);
        delivery_result(self.0.device_id, delivery_id, &ack)
    }

    pub(super) fn retire(&self) {
        self.0.active.store(false, Ordering::Release);
        self.0
            .delivery_authority
            .retire_generation(self.0.queue_generation);
    }
}

impl DisplayOutputLaneInner {
    fn evict_display_sink(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.display_sink = None;
        state.next_display_sink_lookup_at = None;
    }
}

impl DisplayDeliveryCompletion {
    fn record_delivery_ack(&self, ack: &DeviceDeliveryAck) {
        let lane = self.lane.upgrade();
        if ack.id != self.delivery_id {
            let error = DeviceError::protocol(
                self.device_id,
                format!(
                    "display delivery acknowledgement {:?} does not match {:?}",
                    ack.id, self.delivery_id
                ),
            );
            if let Some(lane) = &lane {
                lane.transport_failed.fetch_add(1, Ordering::Relaxed);
                lane.evict_display_sink();
            }
            self.failure_tracker.record_failure(self.delivery_id, error);
            return;
        }

        if ack.transport_started
            && let Some(lane) = &lane
        {
            lane.transport_started.fetch_add(1, Ordering::Relaxed);
        }
        match ack.status {
            DeviceDeliveryStatus::Completed => {
                if let Some(lane) = &lane {
                    lane.transport_completed.fetch_add(1, Ordering::Relaxed);
                }
                self.failure_tracker.record_success(self.delivery_id);
            }
            DeviceDeliveryStatus::Failed => {
                let error = ack.error.clone().unwrap_or_else(|| {
                    DeviceError::protocol(self.device_id, "display delivery failed without error")
                });
                if let Some(lane) = &lane {
                    lane.transport_failed.fetch_add(1, Ordering::Relaxed);
                    lane.evict_display_sink();
                }
                self.failure_tracker.record_failure(self.delivery_id, error);
            }
            DeviceDeliveryStatus::SuppressedDuplicate | DeviceDeliveryStatus::SuppressedCadence => {
                let error = DeviceError::protocol(
                    self.device_id,
                    "display transport returned an unsupported suppressed acknowledgement",
                );
                if let Some(lane) = &lane {
                    lane.transport_failed.fetch_add(1, Ordering::Relaxed);
                    lane.evict_display_sink();
                }
                self.failure_tracker.record_failure(self.delivery_id, error);
            }
        }
    }
}

impl DisplayDeliveryAuthority {
    fn register_generation(
        &self,
        backend_id: String,
        device_id: DeviceId,
        queue_generation: u64,
    ) -> Arc<AsyncWriteFailureTracker> {
        let tracker = Arc::new(AsyncWriteFailureTracker::new(
            backend_id,
            device_id,
            queue_generation,
        ));
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .generations
            .insert(
                queue_generation,
                DisplayDeliveryGeneration {
                    tracker: Arc::clone(&tracker),
                    accepting: true,
                    in_flight: HashMap::new(),
                },
            );
        tracker
    }

    fn begin_delivery(
        &self,
        queue_generation: u64,
        backend: BackendHandle,
    ) -> Option<DeviceDeliveryId> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let generation = state.generations.get_mut(&queue_generation)?;
        if !generation.accepting {
            return None;
        }
        let delivery_id = generation.tracker.begin_delivery();
        generation.in_flight.insert(delivery_id, backend);
        Some(delivery_id)
    }

    fn finish_delivery(&self, delivery_id: DeviceDeliveryId) {
        let backend = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(generation) = state.generations.get_mut(&delivery_id.queue_generation) else {
                return;
            };
            let backend = generation.in_flight.remove(&delivery_id);
            Self::prune_generation(&mut state, delivery_id.queue_generation);
            backend
        };
        drop(backend);
    }

    fn retire_generation(&self, queue_generation: u64) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(generation) = state.generations.get_mut(&queue_generation) {
            generation.accepting = false;
        }
        Self::prune_generation(&mut state, queue_generation);
    }

    pub(super) fn pending_failures(&self) -> Vec<AsyncWriteFailure> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let failures = state
            .generations
            .values()
            .filter_map(|generation| {
                generation.tracker.pending_failure().map(|failure| {
                    if generation.accepting {
                        failure
                    } else {
                        failure.mark_generation_retired()
                    }
                })
            })
            .collect::<Vec<_>>();
        let retired = state
            .generations
            .iter()
            .filter_map(|(queue_generation, generation)| {
                (!generation.accepting
                    && generation.in_flight.is_empty()
                    && generation.tracker.pending_failure().is_none())
                .then_some(*queue_generation)
            })
            .collect::<Vec<_>>();
        for queue_generation in retired {
            state.generations.remove(&queue_generation);
        }
        failures
    }

    pub(super) fn statistics(&self) -> DisplayDeliverySupervisorStatistics {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        DisplayDeliverySupervisorStatistics {
            retained_generations: state.generations.len(),
            in_flight: state
                .generations
                .values()
                .map(|generation| generation.in_flight.len())
                .sum(),
        }
    }

    fn prune_generation(state: &mut DisplayDeliveryAuthorityState, queue_generation: u64) {
        let should_remove = state
            .generations
            .get(&queue_generation)
            .is_some_and(|generation| {
                !generation.accepting
                    && generation.in_flight.is_empty()
                    && generation.tracker.pending_failure().is_none()
            });
        if should_remove {
            state.generations.remove(&queue_generation);
        }
    }
}

impl DisplayDeliveryCompletion {
    fn new(
        lane: Arc<DisplayOutputLaneInner>,
        delivery_id: DeviceDeliveryId,
        delivery_guard: OwnedMutexGuard<()>,
    ) -> Self {
        Self {
            lane: Arc::downgrade(&lane),
            device_id: lane.device_id,
            failure_tracker: Arc::clone(&lane.failure_tracker),
            delivery_authority: Arc::downgrade(&lane.delivery_authority),
            delivery_id,
            delivery_guard: StdMutex::new(Some(delivery_guard)),
            transport_started: AtomicBool::new(false),
            terminal: AtomicBool::new(false),
        }
    }

    fn finish(&self, ack: &DeviceDeliveryAck) {
        if self.terminal.swap(true, Ordering::AcqRel) {
            return;
        }
        self.record_delivery_ack(ack);
        if let Some(delivery_authority) = self.delivery_authority.upgrade() {
            delivery_authority.finish_delivery(self.delivery_id);
        }
        let _ = self
            .delivery_guard
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }
}

impl DeviceDeliveryObserver for DisplayDeliveryCompletion {
    fn transport_started(&self, delivery_id: DeviceDeliveryId) {
        if delivery_id == self.delivery_id {
            self.transport_started.store(true, Ordering::Release);
        }
    }

    fn delivery_terminal(&self, ack: &DeviceDeliveryAck) {
        self.finish(ack);
    }
}

impl Drop for DisplayDeliveryCompletion {
    fn drop(&mut self) {
        if self.terminal.load(Ordering::Acquire) {
            return;
        }
        self.finish(&DeviceDeliveryAck::failed(
            self.delivery_id,
            self.transport_started.load(Ordering::Acquire),
            Duration::ZERO,
            DeviceError::Disconnected {
                device: self.device_id.to_string(),
            },
        ));
    }
}

fn delivery_result(
    device_id: DeviceId,
    delivery_id: DeviceDeliveryId,
    ack: &DeviceDeliveryAck,
) -> Result<(), DeviceError> {
    if ack.id != delivery_id {
        return Err(DeviceError::protocol(
            device_id,
            format!(
                "display delivery acknowledgement {:?} does not match {:?}",
                ack.id, delivery_id
            ),
        ));
    }
    match ack.status {
        DeviceDeliveryStatus::Completed => Ok(()),
        DeviceDeliveryStatus::Failed => Err(ack.error.clone().unwrap_or_else(|| {
            DeviceError::protocol(device_id, "display delivery failed without error")
        })),
        DeviceDeliveryStatus::SuppressedDuplicate | DeviceDeliveryStatus::SuppressedCadence => {
            Err(DeviceError::protocol(
                device_id,
                "display transport returned an unsupported suppressed acknowledgement",
            ))
        }
    }
}

impl BackendManager {
    /// Return the coordinator-owned display output lane for one device.
    pub fn display_output_lane(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
    ) -> Option<DisplayOutputLane> {
        let backend = self.backends.get(backend_id)?.clone();
        let backend_generation = self.backend_generation(backend_id)?;
        Some(
            self.output
                .display_lane(backend_id, device_id, backend, backend_generation),
        )
    }

    /// Return coordinator-owned terminal display delivery supervision state.
    #[must_use]
    pub fn display_delivery_supervisor_statistics(&self) -> DisplayDeliverySupervisorStatistics {
        self.output.display_delivery_authority().statistics()
    }
}
