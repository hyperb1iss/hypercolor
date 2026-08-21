use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use hypercolor_types::device::{DeviceId, OwnedDisplayFramePayload};

use crate::device::traits::DeviceDisplaySink;

use super::{BackendHandle, BackendManager};

/// Cloneable display transport lane owned by the device output coordinator.
#[derive(Clone)]
pub struct DisplayOutputLane(Arc<DisplayOutputLaneInner>);

/// Telemetry for one coordinator-owned display output lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayOutputStatistics {
    /// Generation qualifying this lane's delivery counters.
    pub queue_generation: u64,
    /// Total payloads submitted to the lane.
    pub attempts: u64,
    /// Total payloads delivered successfully.
    pub completed: u64,
    /// Total payloads rejected by the active transport.
    pub failed: u64,
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
    queue_generation: u64,
    attempts: AtomicU64,
    completed: AtomicU64,
    failed: AtomicU64,
}

impl DisplayOutputLane {
    pub(super) fn new(backend: BackendHandle, device_id: DeviceId, queue_generation: u64) -> Self {
        let display_sink = backend.display_sink(&device_id);
        Self(Arc::new(DisplayOutputLaneInner {
            backend,
            device_id,
            state: StdMutex::new(DisplayLaneState {
                display_sink,
                next_display_sink_lookup_at: None,
            }),
            queue_generation,
            attempts: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            failed: AtomicU64::new(0),
        }))
    }

    /// Queue generation shared with LED output telemetry identities.
    #[must_use]
    pub fn queue_generation(&self) -> u64 {
        self.0.queue_generation
    }

    /// Return the current display delivery telemetry snapshot.
    #[must_use]
    pub fn statistics(&self) -> DisplayOutputStatistics {
        DisplayOutputStatistics {
            queue_generation: self.0.queue_generation,
            attempts: self.0.attempts.load(Ordering::Relaxed),
            completed: self.0.completed.load(Ordering::Relaxed),
            failed: self.0.failed.load(Ordering::Relaxed),
        }
    }

    /// Deliver an owned display payload through the current per-device sink.
    ///
    /// A failed sink is evicted before the backend fallback is attempted, so
    /// the next delivery can adopt a replacement sink without stale transport
    /// state outside the lane.
    ///
    /// # Errors
    ///
    /// Returns an error when both the per-device sink and backend fallback fail.
    pub async fn write(&self, payload: Arc<OwnedDisplayFramePayload>) -> Result<()> {
        self.0.attempts.fetch_add(1, Ordering::Relaxed);
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

        if let Some(sink) = sink {
            match sink.write_display_payload_owned(Arc::clone(&payload)).await {
                Ok(()) => {
                    self.0.completed.fetch_add(1, Ordering::Relaxed);
                    return Ok(());
                }
                Err(sink_error) => {
                    let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
                    state.display_sink = None;
                    state.next_display_sink_lookup_at = None;
                    drop(state);
                    self.0.failed.fetch_add(1, Ordering::Relaxed);
                    return Err(sink_error).context("display sink delivery failed");
                }
            }
        }

        let result = self
            .0
            .backend
            .write_display_payload_owned(&self.0.device_id, payload)
            .await;
        if result.is_ok() {
            self.0.completed.fetch_add(1, Ordering::Relaxed);
        } else {
            self.0.failed.fetch_add(1, Ordering::Relaxed);
        }
        result.context("display backend delivery failed")
    }
}

impl BackendManager {
    /// Write one immediate JPEG display payload to a specific physical device.
    ///
    /// This bypasses spatial routing and targets display-capable backends
    /// directly for screen/LCD updates.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend is missing or the backend write fails.
    pub async fn write_device_display_frame(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        jpeg_data: &[u8],
    ) -> Result<()> {
        let Some(lane) = self.display_output_lane(backend_id, device_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        lane.write(Arc::new(OwnedDisplayFramePayload::jpeg(
            0,
            0,
            Arc::new(jpeg_data.to_vec()),
        )))
        .await
    }

    /// Write one owned JPEG display payload to a specific physical device.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend is missing or the backend write fails.
    pub async fn write_device_display_frame_owned(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        jpeg_data: Arc<Vec<u8>>,
    ) -> Result<()> {
        let Some(lane) = self.display_output_lane(backend_id, device_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        lane.write(Arc::new(OwnedDisplayFramePayload::jpeg(0, 0, jpeg_data)))
            .await
    }

    /// Write one owned display payload to a specific physical device.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend is missing or the backend write fails.
    pub async fn write_device_display_payload_owned(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> Result<()> {
        let Some(lane) = self.display_output_lane(backend_id, device_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        lane.write(payload).await
    }

    /// Return the coordinator-owned display output lane for one device.
    pub fn display_output_lane(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
    ) -> Option<DisplayOutputLane> {
        let backend = self.backends.get(backend_id)?.clone();
        Some(self.output.display_lane(backend_id, device_id, backend))
    }
}
