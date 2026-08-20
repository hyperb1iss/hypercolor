//! Relay tasks that pump events, frames, spectrum, canvases, and metrics
//! from the render bus out to connected WebSocket clients.
//!
//! Each relay owns its own `tokio::task` and watches an immutable
//! `SubscriptionState` snapshot. Slow consumers are handled with bounded
//! queues according to the topic registry: awaited lossless sends,
//! latest-value replacement, or drops paired with a backpressure notice.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use axum::body::Bytes;
use axum::extract::ws::Utf8Bytes;
use hypercolor_core::bus::EventTimestamp;
use hypercolor_core::device::usb_actor_metrics_snapshot;
use hypercolor_core::engine::RenderLoopState;
use hypercolor_core::input::BrowserInputPublicationId;
use hypercolor_leptos_ext::ws::registry::{
    Cadence, CanvasConfig, CanvasFormat, DisplayPreviewConfig, FramesConfig, METRICS_FPS_MIN,
    MetricsConfig, ScreenZonesConfig, SpectrumConfig, TopicId,
};
use hypercolor_leptos_ext::ws::{
    DisplayPreviewFrame as WireDisplayPreviewFrame,
    InteractivePreviewFrame as WireInteractivePreviewFrame, PREVIEW_CHUNK_FIXED_HEADER_LEN,
    PreviewCancelFrame, PreviewChunkFrame, PreviewFrame as WirePreviewFrame, PreviewFrameChannel,
    PreviewPixelFormat as WirePreviewFormat, PreviewPublicationMetadata, PreviewStreamId,
    PreviewTransportLimits, ScreenZonesFrame as WireScreenZonesFrame,
    ZonePreviewFrame as WireZonePreviewFrame,
};
use hypercolor_types::canvas::{PublishedSurfaceStorageIdentity, SurfaceDescriptor};
use hypercolor_types::event::HypercolorEvent;
use hypercolor_types::sensor::SystemSnapshot;
use thiserror::Error;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{Notify, broadcast, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use super::cache::{
    WS_CANVAS_BYTES_PER_PIXEL_RGBA, WS_CANVAS_PAYLOAD_BUILD_COUNT,
    WS_CANVAS_PAYLOAD_CACHE_HIT_COUNT, WS_CLIENT_COUNT, WS_FRAME_PAYLOAD_BUILD_COUNT,
    WS_FRAME_PAYLOAD_CACHE_HIT_COUNT, WS_SCREEN_CANVAS_HEADER, WS_TOTAL_BYTES_SENT,
    WS_WEB_VIEWPORT_CANVAS_HEADER, cached_display_preview_payload, cached_frame_payload,
    cached_spectrum_payload, try_encode_cached_canvas_binary_with_header_scaled,
    try_encode_cached_canvas_preview_binary, try_encode_cached_zone_preview_binary_scaled,
};
use super::protocol::{
    ActiveFramesConfig, MetricsCopies, MetricsDevices, MetricsDisplayLane, MetricsDisplayOutput,
    MetricsEffectHealth, MetricsFps, MetricsFrameTime, MetricsMemory, MetricsPacing,
    MetricsPayload, MetricsPreview, MetricsPreviewDemand, MetricsRenderSurfaces,
    MetricsSessionLatency, MetricsStages, MetricsTimeline, MetricsWebsocket, ServerMessage,
    SubscriptionState, event_message_parts, should_relay_event,
};
use crate::api::AppState;
use crate::interactive_preview::PreviewResourceLease;
use crate::performance::FrameTimeSummary as RenderFrameTimeSummary;
use crate::performance::LatestFrameMetrics;
use crate::preview_runtime::{PreviewDemandSummary, PreviewPixelFormat, PreviewStreamDemand};
use crate::session::OutputPowerState;

const BACKPRESSURE_REPORT_INTERVAL: Duration = Duration::from_millis(500);
pub(super) static WS_PREVIEW_PUBLICATION_QUEUED_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_PREVIEW_PUBLICATION_REPLACED_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_PREVIEW_PUBLICATION_EVICTED_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_PREVIEW_PUBLICATION_REJECTED_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_PREVIEW_PUBLICATION_SENT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_PREVIEW_CHUNK_SENT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static WS_PREVIEW_QUEUE_BYTES: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy)]
pub(super) struct PreviewOutboundLimits {
    pub(super) max_publication_bytes: usize,
    pub(super) max_connection_bytes: usize,
}

impl Default for PreviewOutboundLimits {
    fn default() -> Self {
        let limits = PreviewTransportLimits::default();
        Self {
            max_publication_bytes: limits.max_encoded_publication_bytes,
            max_connection_bytes: limits.max_connection_bytes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreviewPublishOutcome {
    Queued,
    Replaced,
}

#[derive(Debug, Error)]
pub(super) enum PreviewOutboundError {
    #[error("preview publication is invalid: {0}")]
    InvalidPublication(String),
    #[error("preview publication stream does not match its wire frame")]
    StreamMismatch,
    #[error("interactive preview publication is missing its input-publication fence")]
    MissingInteractiveFence,
    #[error("passive preview publication unexpectedly carries an interactive fence")]
    UnexpectedInteractiveFence,
    #[error("preview publication uses {actual} bytes, exceeding the {maximum}-byte budget")]
    PublicationBudgetExceeded { maximum: usize, actual: usize },
    #[error("decoded preview uses {actual} bytes, exceeding the {maximum}-byte budget")]
    DecodedPublicationBudgetExceeded { maximum: usize, actual: usize },
    #[error(
        "preview connection queue cannot admit a {actual}-byte publication within {maximum} bytes"
    )]
    ConnectionBudgetExceeded { maximum: usize, actual: usize },
    #[error("preview connection retains {retained} bytes; {requested} more must wait")]
    ConnectionBusy { retained: usize, requested: usize },
    #[error("preview sender state needs {actual} bytes; limit is {maximum}")]
    SenderStateBudgetExceeded { maximum: usize, actual: usize },
    #[error("preview cursor state needs {actual} bytes; limit is {maximum}")]
    CursorStateBudgetExceeded { maximum: usize, actual: usize },
    #[error("preview router could not allocate indexed state for {entries} streams")]
    RouterAllocationFailed { entries: usize },
    #[error("preview publication identity space is exhausted")]
    PublicationIdExhausted,
    #[error("preview chunk encoding failed: {0}")]
    ChunkEncoding(String),
}

#[derive(Debug)]
pub(super) struct PreviewPublication {
    metadata: PreviewPublicationMetadata,
    encoded: Bytes,
    interactive_fence: Option<BrowserInputPublicationId>,
    _resource_guard: Option<PreviewResourceLease>,
}

impl PreviewPublication {
    pub(super) fn stream(&self) -> &PreviewStreamId {
        &self.metadata.stream
    }

    pub(super) const fn publication_id(&self) -> u64 {
        self.metadata.publication_id
    }

    fn key(&self) -> PreviewPublicationKey {
        PreviewPublicationKey {
            stream: self.metadata.stream.clone(),
            publication_id: self.metadata.publication_id,
        }
    }

    pub(super) fn interactive_fence(&self) -> Option<(&str, BrowserInputPublicationId)> {
        match (&self.metadata.stream, self.interactive_fence) {
            (PreviewStreamId::Interactive(preview_id), Some(publication_id)) => {
                Some((preview_id, publication_id))
            }
            _ => None,
        }
    }
}

#[derive(Debug)]
struct PreviewOutboundState {
    queued: HashMap<PreviewStreamId, QueuedPreviewPublication>,
    queue_head: Option<PreviewStreamId>,
    queue_tail: Option<PreviewStreamId>,
    queued_bytes: usize,
    in_flight: HashMap<PreviewPublicationKey, usize>,
    in_flight_bytes: usize,
    current: HashMap<PreviewStreamId, u64>,
    pending_cancellations: HashMap<PreviewStreamId, u64>,
    cancellation_order: VecDeque<PreviewStreamId>,
    next_publication_id: u64,
    limits: PreviewOutboundLimits,
    transport_limits: PreviewTransportLimits,
}

impl Drop for PreviewOutboundState {
    fn drop(&mut self) {
        WS_PREVIEW_QUEUE_BYTES.fetch_sub(self.retained_bytes(), Ordering::Relaxed);
    }
}

impl PreviewOutboundState {
    fn retained_bytes(&self) -> usize {
        self.queued_bytes.saturating_add(self.in_flight_bytes)
    }

    fn sender_state_bytes(&self) -> usize {
        let queued = self
            .queued
            .keys()
            .map(preview_queued_state_bytes)
            .fold(0_usize, usize::saturating_add);
        let in_flight = self
            .in_flight
            .keys()
            .map(|key| preview_in_flight_state_bytes(&key.stream))
            .fold(0_usize, usize::saturating_add);
        let current = self
            .current
            .keys()
            .map(preview_current_state_bytes)
            .fold(0_usize, usize::saturating_add);
        let cancellations = self
            .pending_cancellations
            .keys()
            .map(preview_cancellation_state_bytes)
            .fold(0_usize, usize::saturating_add);
        queued
            .saturating_add(in_flight)
            .saturating_add(current)
            .saturating_add(cancellations)
    }

    fn try_reserve_stream_state(
        &mut self,
        stream: &PreviewStreamId,
    ) -> Result<(), PreviewOutboundError> {
        let entries = self.current.len().saturating_add(1);
        self.queued
            .try_reserve(1)
            .map_err(|_| PreviewOutboundError::RouterAllocationFailed { entries })?;
        let prospective_queued = self
            .queued
            .len()
            .saturating_add(usize::from(!self.queued.contains_key(stream)));
        self.in_flight
            .try_reserve(prospective_queued)
            .map_err(|_| PreviewOutboundError::RouterAllocationFailed { entries })?;
        if !self.current.contains_key(stream) {
            self.current
                .try_reserve(1)
                .map_err(|_| PreviewOutboundError::RouterAllocationFailed { entries })?;
        }
        let additional = usize::from(!self.current.contains_key(stream))
            .saturating_mul(preview_current_state_bytes(stream))
            .saturating_add(
                usize::from(!self.queued.contains_key(stream))
                    .saturating_mul(preview_queued_state_bytes(stream)),
            );
        let requested = self.sender_state_bytes().saturating_add(additional);
        if requested > self.transport_limits.max_sender_state_bytes {
            return Err(PreviewOutboundError::SenderStateBudgetExceeded {
                maximum: self.transport_limits.max_sender_state_bytes,
                actual: requested,
            });
        }
        Ok(())
    }

    fn try_reserve_cancellations(
        &mut self,
        additional: usize,
        additional_bytes: usize,
    ) -> Result<(), PreviewOutboundError> {
        let entries = self.pending_cancellations.len().saturating_add(additional);
        let requested = self.sender_state_bytes().saturating_add(additional_bytes);
        if requested > self.transport_limits.max_sender_state_bytes {
            return Err(PreviewOutboundError::SenderStateBudgetExceeded {
                maximum: self.transport_limits.max_sender_state_bytes,
                actual: requested,
            });
        }
        self.pending_cancellations
            .try_reserve(additional)
            .map_err(|_| PreviewOutboundError::RouterAllocationFailed { entries })?;
        self.cancellation_order
            .try_reserve(additional)
            .map_err(|_| PreviewOutboundError::RouterAllocationFailed { entries })
    }

    fn record_cancellation(&mut self, stream: PreviewStreamId, publication_id: u64) {
        if let Some(existing) = self.pending_cancellations.get_mut(&stream) {
            *existing = (*existing).max(publication_id);
            return;
        }
        self.cancellation_order.push_back(stream.clone());
        self.pending_cancellations.insert(stream, publication_id);
    }

    fn pop_cancellation(&mut self) -> Option<PreviewCancelFrame> {
        let stream = self.cancellation_order.pop_front()?;
        let publication_id = self
            .pending_cancellations
            .remove(&stream)
            .expect("cancellation order must reference indexed state");
        Some(PreviewCancelFrame {
            stream,
            publication_id,
        })
    }

    fn enqueue(&mut self, publication: PreviewPublication) -> Option<PreviewPublication> {
        let stream = publication.stream().clone();
        let replaced = self.remove_queued(&stream);

        let previous = self.queue_tail.clone();
        if let Some(tail) = &previous {
            self.queued
                .get_mut(tail)
                .expect("queue tail must reference an indexed publication")
                .next = Some(stream.clone());
        } else {
            self.queue_head = Some(stream.clone());
        }
        self.queue_tail = Some(stream.clone());
        self.queued.insert(
            stream,
            QueuedPreviewPublication {
                publication,
                previous,
                next: None,
            },
        );
        replaced
    }

    fn remove_queued(&mut self, stream: &PreviewStreamId) -> Option<PreviewPublication> {
        let queued = self.queued.remove(stream)?;
        if let Some(previous) = &queued.previous {
            self.queued
                .get_mut(previous)
                .expect("queued predecessor must remain indexed")
                .next
                .clone_from(&queued.next);
        } else {
            self.queue_head.clone_from(&queued.next);
        }
        if let Some(next) = &queued.next {
            self.queued
                .get_mut(next)
                .expect("queued successor must remain indexed")
                .previous
                .clone_from(&queued.previous);
        } else {
            self.queue_tail.clone_from(&queued.previous);
        }
        Some(queued.publication)
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct PreviewPublicationKey {
    stream: PreviewStreamId,
    publication_id: u64,
}

#[derive(Debug)]
struct QueuedPreviewPublication {
    publication: PreviewPublication,
    previous: Option<PreviewStreamId>,
    next: Option<PreviewStreamId>,
}

fn preview_queued_state_bytes(stream: &PreviewStreamId) -> usize {
    std::mem::size_of::<(PreviewStreamId, QueuedPreviewPublication)>()
        .saturating_add(stream.identity_bytes().saturating_mul(4))
}

fn preview_in_flight_state_bytes(stream: &PreviewStreamId) -> usize {
    std::mem::size_of::<(PreviewPublicationKey, usize)>().saturating_add(stream.identity_bytes())
}

fn preview_current_state_bytes(stream: &PreviewStreamId) -> usize {
    std::mem::size_of::<(PreviewStreamId, u64)>().saturating_add(stream.identity_bytes())
}

fn preview_cancellation_state_bytes(stream: &PreviewStreamId) -> usize {
    std::mem::size_of::<(PreviewStreamId, u64)>()
        .saturating_add(std::mem::size_of::<PreviewStreamId>())
        .saturating_add(stream.identity_bytes().saturating_mul(2))
}

#[derive(Debug)]
struct PreviewOutboundShared {
    state: StdMutex<PreviewOutboundState>,
    item_notify: Notify,
    capacity_notify: Notify,
}

#[derive(Clone, Debug)]
pub(super) struct PreviewOutboundSender {
    shared: Arc<PreviewOutboundShared>,
}

#[derive(Debug)]
pub(super) struct PreviewOutboundReceiver {
    shared: Arc<PreviewOutboundShared>,
}

pub(super) fn preview_outbound_channel() -> (PreviewOutboundSender, PreviewOutboundReceiver) {
    preview_outbound_channel_with_limits(PreviewOutboundLimits::default())
}

pub(super) fn preview_outbound_channel_with_limits(
    limits: PreviewOutboundLimits,
) -> (PreviewOutboundSender, PreviewOutboundReceiver) {
    let protocol_limits = PreviewTransportLimits::default();
    let shared = Arc::new(PreviewOutboundShared {
        state: StdMutex::new(PreviewOutboundState {
            queued: HashMap::new(),
            queue_head: None,
            queue_tail: None,
            queued_bytes: 0,
            in_flight: HashMap::new(),
            in_flight_bytes: 0,
            current: HashMap::new(),
            pending_cancellations: HashMap::new(),
            cancellation_order: VecDeque::new(),
            next_publication_id: 1,
            limits,
            transport_limits: PreviewTransportLimits {
                max_encoded_publication_bytes: protocol_limits
                    .max_encoded_publication_bytes
                    .min(limits.max_publication_bytes),
                max_connection_bytes: protocol_limits
                    .max_connection_bytes
                    .min(limits.max_connection_bytes),
                ..protocol_limits
            },
        }),
        item_notify: Notify::new(),
        capacity_notify: Notify::new(),
    });
    (
        PreviewOutboundSender {
            shared: Arc::clone(&shared),
        },
        PreviewOutboundReceiver { shared },
    )
}

impl PreviewOutboundSender {
    pub(super) fn publish(
        &self,
        stream: PreviewStreamId,
        encoded: Bytes,
        interactive_fence: Option<BrowserInputPublicationId>,
    ) -> Result<PreviewPublishOutcome, PreviewOutboundError> {
        self.publish_with_optional_resource_guard(stream, encoded, interactive_fence, None)
    }

    pub(super) fn publish_with_resource_guard(
        &self,
        stream: PreviewStreamId,
        encoded: Bytes,
        interactive_fence: Option<BrowserInputPublicationId>,
        resource_guard: PreviewResourceLease,
    ) -> Result<PreviewPublishOutcome, PreviewOutboundError> {
        self.publish_with_optional_resource_guard(
            stream,
            encoded,
            interactive_fence,
            Some(resource_guard),
        )
    }

    fn publish_with_optional_resource_guard(
        &self,
        stream: PreviewStreamId,
        encoded: Bytes,
        interactive_fence: Option<BrowserInputPublicationId>,
        resource_guard: Option<PreviewResourceLease>,
    ) -> Result<PreviewPublishOutcome, PreviewOutboundError> {
        let result = self.publish_inner(stream, encoded, interactive_fence, resource_guard);
        if result.is_err() {
            WS_PREVIEW_PUBLICATION_REJECTED_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    fn publish_inner(
        &self,
        stream: PreviewStreamId,
        encoded: Bytes,
        interactive_fence: Option<BrowserInputPublicationId>,
        resource_guard: Option<PreviewResourceLease>,
    ) -> Result<PreviewPublishOutcome, PreviewOutboundError> {
        validate_preview_fence(&stream, interactive_fence)?;
        let fields = decode_preview_fields(&stream, &encoded)?;
        let decoded_bytes = decoded_preview_bytes(&stream, fields.width, fields.height)?;
        let encoded_len = encoded.len();
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if decoded_bytes > state.transport_limits.max_decoded_publication_bytes {
            return Err(PreviewOutboundError::DecodedPublicationBudgetExceeded {
                maximum: state.transport_limits.max_decoded_publication_bytes,
                actual: decoded_bytes,
            });
        }
        if encoded.len() > state.limits.max_publication_bytes {
            return Err(PreviewOutboundError::PublicationBudgetExceeded {
                maximum: state.limits.max_publication_bytes,
                actual: encoded.len(),
            });
        }
        if encoded.len() > state.limits.max_connection_bytes {
            return Err(PreviewOutboundError::ConnectionBudgetExceeded {
                maximum: state.limits.max_connection_bytes,
                actual: encoded.len(),
            });
        }
        let replaced_bytes = state
            .queued
            .get(&stream)
            .map_or(0, |queued| queued.publication.encoded.len());
        let projected_bytes = state
            .retained_bytes()
            .checked_sub(replaced_bytes)
            .and_then(|bytes| bytes.checked_add(encoded.len()))
            .ok_or(PreviewOutboundError::ConnectionBudgetExceeded {
                maximum: state.limits.max_connection_bytes,
                actual: encoded.len(),
            })?;
        if projected_bytes > state.limits.max_connection_bytes {
            return Err(PreviewOutboundError::ConnectionBusy {
                retained: state.retained_bytes().saturating_sub(replaced_bytes),
                requested: encoded.len(),
            });
        }
        let cancellation_reservations = usize::from(
            state.current.contains_key(&stream)
                && !state.pending_cancellations.contains_key(&stream),
        );
        let cancellation_reservation_bytes = if cancellation_reservations == 0 {
            0
        } else {
            preview_cancellation_state_bytes(&stream)
        };
        state.try_reserve_stream_state(&stream)?;
        state
            .try_reserve_cancellations(cancellation_reservations, cancellation_reservation_bytes)?;

        let publication_id = state.next_publication_id;
        let next_publication_id = publication_id
            .checked_add(1)
            .ok_or(PreviewOutboundError::PublicationIdExhausted)?;
        let publication = PreviewPublication {
            metadata: PreviewPublicationMetadata {
                stream: stream.clone(),
                publication_id,
                frame_number: fields.frame_number,
                timestamp_ms: fields.timestamp_ms,
                width: fields.width,
                height: fields.height,
                format: fields.format,
            },
            encoded,
            interactive_fence,
            _resource_guard: resource_guard,
        };
        if let Some(previous_publication_id) = state.current.get(&stream).copied() {
            state.record_cancellation(stream.clone(), previous_publication_id);
        }
        let replaced = state.enqueue(publication);
        let outcome = if let Some(replaced) = replaced {
            state.queued_bytes = state.queued_bytes.saturating_sub(replaced.encoded.len());
            WS_PREVIEW_QUEUE_BYTES.fetch_sub(replaced.encoded.len(), Ordering::Relaxed);
            WS_PREVIEW_PUBLICATION_REPLACED_COUNT.fetch_add(1, Ordering::Relaxed);
            PreviewPublishOutcome::Replaced
        } else {
            PreviewPublishOutcome::Queued
        };

        set_current_publication(&mut state.current, &stream, publication_id);
        state.next_publication_id = next_publication_id;
        state.queued_bytes += encoded_len;
        WS_PREVIEW_QUEUE_BYTES.fetch_add(encoded_len, Ordering::Relaxed);
        WS_PREVIEW_PUBLICATION_QUEUED_COUNT.fetch_add(1, Ordering::Relaxed);
        drop(state);
        self.shared.item_notify.notify_one();
        Ok(outcome)
    }

    pub(super) fn cancel(&self, stream: &PreviewStreamId) -> Result<bool, PreviewOutboundError> {
        Ok(self.cancel_many(std::slice::from_ref(stream))? > 0)
    }

    pub(super) fn cancel_many(
        &self,
        requested: &[PreviewStreamId],
    ) -> Result<usize, PreviewOutboundError> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut seen = HashSet::new();
        let streams = requested
            .iter()
            .filter(|stream| seen.insert((*stream).clone()) && state.current.contains_key(*stream))
            .cloned()
            .collect::<Vec<_>>();
        let additional = streams
            .iter()
            .filter(|stream| !state.pending_cancellations.contains_key(*stream))
            .count();
        let additional_bytes = streams
            .iter()
            .filter(|stream| !state.pending_cancellations.contains_key(*stream))
            .map(preview_cancellation_state_bytes)
            .fold(0_usize, usize::saturating_add);
        state.try_reserve_cancellations(additional, additional_bytes)?;
        for stream in &streams {
            let publication_id = state
                .current
                .remove(stream)
                .expect("matched cancellation stream must remain current");
            if let Some(removed) = state.remove_queued(stream) {
                state.queued_bytes = state.queued_bytes.saturating_sub(removed.encoded.len());
                WS_PREVIEW_QUEUE_BYTES.fetch_sub(removed.encoded.len(), Ordering::Relaxed);
            }
            state.record_cancellation(stream.clone(), publication_id);
        }
        let cancelled = streams.len();
        drop(state);
        if cancelled > 0 {
            self.shared.item_notify.notify_one();
            self.shared.capacity_notify.notify_waiters();
        }
        Ok(cancelled)
    }

    pub(super) fn discard_unsent(&self, stream: &PreviewStreamId) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let released_capacity = if let Some(removed) = state.remove_queued(stream) {
            state.queued_bytes = state.queued_bytes.saturating_sub(removed.encoded.len());
            WS_PREVIEW_QUEUE_BYTES.fetch_sub(removed.encoded.len(), Ordering::Relaxed);
            true
        } else {
            false
        };
        state.current.remove(stream);
        drop(state);
        if released_capacity {
            self.shared.capacity_notify.notify_waiters();
        }
    }

    pub(super) fn cancel_subscription(
        &self,
        topic: TopicId,
        key: Option<&str>,
    ) -> Result<usize, PreviewOutboundError> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut streams = Vec::new();
        streams.try_reserve(state.current.len()).map_err(|_| {
            PreviewOutboundError::RouterAllocationFailed {
                entries: state.current.len(),
            }
        })?;
        streams.extend(
            state
                .current
                .keys()
                .filter(|stream| preview_stream_matches_selection(stream, topic, key))
                .cloned(),
        );
        let additional = streams
            .iter()
            .filter(|stream| !state.pending_cancellations.contains_key(*stream))
            .count();
        let additional_bytes = streams
            .iter()
            .filter(|stream| !state.pending_cancellations.contains_key(*stream))
            .map(preview_cancellation_state_bytes)
            .fold(0_usize, usize::saturating_add);
        state.try_reserve_cancellations(additional, additional_bytes)?;
        for stream in &streams {
            let publication_id = state
                .current
                .remove(stream)
                .expect("matched cancellation stream must remain current");
            if let Some(removed) = state.remove_queued(stream) {
                state.queued_bytes = state.queued_bytes.saturating_sub(removed.encoded.len());
                WS_PREVIEW_QUEUE_BYTES.fetch_sub(removed.encoded.len(), Ordering::Relaxed);
            }
            state.record_cancellation(stream.clone(), publication_id);
        }
        let cancelled = streams.len();
        drop(state);
        if cancelled > 0 {
            self.shared.item_notify.notify_one();
            self.shared.capacity_notify.notify_waiters();
        }
        Ok(cancelled)
    }
}

#[derive(Debug)]
pub(super) enum PreviewOutboundItem {
    Publication(PreviewPublication),
    Cancellation(PreviewCancelFrame),
}

/// Whether a live preview stream belongs to one subscription. A `None`
/// key on a keyed topic means "every key", which is what a topic-wide
/// teardown needs.
fn preview_stream_matches_selection(
    stream: &PreviewStreamId,
    topic: TopicId,
    key: Option<&str>,
) -> bool {
    match (stream, topic) {
        (PreviewStreamId::Passive(frame_channel), TopicId::Canvas) => {
            matches!(frame_channel, PreviewFrameChannel::Canvas)
        }
        (PreviewStreamId::Passive(frame_channel), TopicId::ScreenCanvas) => {
            matches!(frame_channel, PreviewFrameChannel::ScreenCanvas)
        }
        (PreviewStreamId::Passive(frame_channel), TopicId::WebViewportCanvas) => {
            matches!(frame_channel, PreviewFrameChannel::WebViewportCanvas)
        }
        (PreviewStreamId::Display(device_id), TopicId::DisplayPreview) => {
            key.is_none_or(|wanted| wanted == device_id)
        }
        (PreviewStreamId::Interactive(preview_id), TopicId::InteractivePreview) => {
            key.is_none_or(|wanted| wanted == preview_id)
        }
        (PreviewStreamId::ScreenZones, TopicId::ScreenZones)
        | (PreviewStreamId::Zone { .. }, TopicId::ZonePreview) => true,
        _ => false,
    }
}

impl PreviewOutboundReceiver {
    pub(super) async fn recv(&self) -> PreviewOutboundItem {
        loop {
            let notified = self.shared.item_notify.notified();
            if let Some(publication) = self.try_recv() {
                return publication;
            }
            notified.await;
        }
    }

    pub(super) fn try_recv(&self) -> Option<PreviewOutboundItem> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(cancellation) = state.pop_cancellation() {
            return Some(PreviewOutboundItem::Cancellation(cancellation));
        }
        let stream = state.queue_head.clone()?;
        let publication = state
            .remove_queued(&stream)
            .expect("queue head must remain indexed");
        let byte_len = publication.encoded.len();
        state.queued_bytes = state.queued_bytes.saturating_sub(byte_len);
        state.in_flight_bytes = state.in_flight_bytes.saturating_add(byte_len);
        state.in_flight.insert(publication.key(), byte_len);
        Some(PreviewOutboundItem::Publication(publication))
    }

    pub(super) fn is_current(&self, publication: &PreviewPublication) -> bool {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .current
            .get(publication.stream())
            .is_some_and(|publication_id| *publication_id == publication.publication_id())
    }

    pub(super) fn complete(&self, publication: &PreviewPublication) {
        self.complete_publication(publication.stream(), publication.publication_id());
    }

    pub(super) fn complete_publication(&self, stream: &PreviewStreamId, publication_id: u64) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let key = PreviewPublicationKey {
            stream: stream.clone(),
            publication_id,
        };
        if let Some(byte_len) = state.in_flight.remove(&key) {
            state.in_flight_bytes = state.in_flight_bytes.saturating_sub(byte_len);
            WS_PREVIEW_QUEUE_BYTES.fetch_sub(byte_len, Ordering::Relaxed);
        }
        remove_current_publication(&mut state.current, stream, publication_id);
        drop(state);
        self.shared.capacity_notify.notify_waiters();
    }
}

#[derive(Debug)]
pub(super) struct PreviewSendCursor {
    publication: PreviewPublication,
    payload_capacity: usize,
    next_offset: usize,
    next_chunk_index: u32,
    chunk_count: u32,
    chunked: bool,
}

impl PreviewSendCursor {
    #[cfg(test)]
    pub(super) fn new(
        publication: PreviewPublication,
        max_message_bytes: usize,
    ) -> Result<Self, PreviewOutboundError> {
        let limits = PreviewTransportLimits::default();
        if max_message_bytes > limits.max_message_bytes {
            return Err(PreviewOutboundError::ChunkEncoding(format!(
                "message budget {max_message_bytes} exceeds protocol limit {}",
                limits.max_message_bytes
            )));
        }
        Self::with_limits(
            publication,
            PreviewTransportLimits {
                max_message_bytes,
                ..limits
            },
        )
    }

    pub(super) fn with_limits(
        publication: PreviewPublication,
        limits: PreviewTransportLimits,
    ) -> Result<Self, PreviewOutboundError> {
        let max_message_bytes = limits.max_message_bytes;
        let identity_len = publication.stream().identity_bytes();
        let envelope_len = PREVIEW_CHUNK_FIXED_HEADER_LEN
            .checked_add(identity_len)
            .ok_or_else(|| {
                PreviewOutboundError::ChunkEncoding("envelope length overflow".into())
            })?;
        let chunked = publication.encoded.len() > max_message_bytes;
        let payload_capacity = if chunked {
            max_message_bytes
                .checked_sub(envelope_len)
                .filter(|bytes| *bytes > 0)
                .ok_or_else(|| {
                    PreviewOutboundError::ChunkEncoding(
                        "message budget cannot fit chunk envelope".into(),
                    )
                })?
        } else {
            publication.encoded.len()
        };
        let chunk_count = if chunked {
            u32::try_from(publication.encoded.len().div_ceil(payload_capacity)).map_err(|_| {
                PreviewOutboundError::ChunkEncoding("chunk count exceeds u32".into())
            })?
        } else {
            1
        };
        let max_chunk_count = limits.effective_max_chunk_count(identity_len);
        if chunk_count > max_chunk_count {
            return Err(PreviewOutboundError::ChunkEncoding(format!(
                "chunk count {chunk_count} exceeds protocol limit {}",
                max_chunk_count
            )));
        }
        Ok(Self {
            publication,
            payload_capacity,
            next_offset: 0,
            next_chunk_index: 0,
            chunk_count,
            chunked,
        })
    }

    pub(super) const fn publication(&self) -> &PreviewPublication {
        &self.publication
    }

    pub(super) fn next_message(&mut self) -> Result<Option<Bytes>, PreviewOutboundError> {
        if self.next_offset >= self.publication.encoded.len() {
            return Ok(None);
        }
        if !self.chunked {
            self.next_offset = self.publication.encoded.len();
            return Ok(Some(self.publication.encoded.clone()));
        }
        let end = self
            .next_offset
            .checked_add(self.payload_capacity)
            .map_or(self.publication.encoded.len(), |end| {
                end.min(self.publication.encoded.len())
            });
        let message = PreviewChunkFrame {
            metadata: self.publication.metadata.clone(),
            total_encoded_bytes: u64::try_from(self.publication.encoded.len()).map_err(|_| {
                PreviewOutboundError::ChunkEncoding("publication length exceeds u64".into())
            })?,
            chunk_offset: u64::try_from(self.next_offset).map_err(|_| {
                PreviewOutboundError::ChunkEncoding("chunk offset exceeds u64".into())
            })?,
            chunk_index: self.next_chunk_index,
            chunk_count: self.chunk_count,
            payload: self.publication.encoded.slice(self.next_offset..end),
        }
        .try_encode()
        .map_err(|error| PreviewOutboundError::ChunkEncoding(error.to_string()))?;
        self.next_offset = end;
        self.next_chunk_index += 1;
        Ok(Some(message))
    }

    pub(super) fn is_complete(&self) -> bool {
        self.next_offset >= self.publication.encoded.len()
    }

    pub(super) const fn is_chunked(&self) -> bool {
        self.chunked
    }
}

#[derive(Debug)]
struct QueuedPreviewCursor {
    cursor: PreviewSendCursor,
    previous: Option<PreviewStreamId>,
    next: Option<PreviewStreamId>,
}

#[derive(Debug)]
pub(super) struct PreviewCursorQueue {
    cursors: HashMap<PreviewStreamId, QueuedPreviewCursor>,
    head: Option<PreviewStreamId>,
    tail: Option<PreviewStreamId>,
    max_state_bytes: usize,
    state_bytes: usize,
}

impl PreviewCursorQueue {
    pub(super) fn with_limits(limits: PreviewTransportLimits) -> Self {
        Self {
            cursors: HashMap::new(),
            head: None,
            tail: None,
            max_state_bytes: limits.max_cursor_state_bytes,
            state_bytes: 0,
        }
    }

    pub(super) fn try_insert(
        &mut self,
        cursor: PreviewSendCursor,
    ) -> Result<Option<PreviewSendCursor>, PreviewOutboundError> {
        let stream = cursor.publication().stream().clone();
        let replacing = self.cursors.contains_key(&stream);
        let replaced_bytes = self.cursors.get(&stream).map_or(0, |queued| {
            preview_cursor_state_bytes(queued.cursor.publication().stream())
        });
        let requested = self
            .state_bytes
            .checked_sub(replaced_bytes)
            .and_then(|bytes| bytes.checked_add(preview_cursor_state_bytes(&stream)))
            .ok_or(PreviewOutboundError::CursorStateBudgetExceeded {
                maximum: self.max_state_bytes,
                actual: usize::MAX,
            })?;
        if requested > self.max_state_bytes {
            return Err(PreviewOutboundError::CursorStateBudgetExceeded {
                maximum: self.max_state_bytes,
                actual: requested,
            });
        }
        if !replacing {
            self.cursors.try_reserve(1).map_err(|_| {
                PreviewOutboundError::RouterAllocationFailed {
                    entries: self.cursors.len().saturating_add(1),
                }
            })?;
        }
        let replaced = self.remove(&stream);
        self.insert_at_tail(stream, cursor);
        self.state_bytes = requested;
        Ok(replaced)
    }

    fn insert_at_tail(&mut self, stream: PreviewStreamId, cursor: PreviewSendCursor) {
        let previous = self.tail.clone();
        if let Some(tail) = &previous {
            self.cursors
                .get_mut(tail)
                .expect("cursor tail must remain indexed")
                .next = Some(stream.clone());
        } else {
            self.head = Some(stream.clone());
        }
        self.tail = Some(stream.clone());
        self.cursors.insert(
            stream,
            QueuedPreviewCursor {
                cursor,
                previous,
                next: None,
            },
        );
    }

    pub(super) fn remove(&mut self, stream: &PreviewStreamId) -> Option<PreviewSendCursor> {
        let queued = self.cursors.remove(stream)?;
        self.state_bytes = self.state_bytes.saturating_sub(preview_cursor_state_bytes(
            queued.cursor.publication().stream(),
        ));
        if let Some(previous) = &queued.previous {
            self.cursors
                .get_mut(previous)
                .expect("cursor predecessor must remain indexed")
                .next
                .clone_from(&queued.next);
        } else {
            self.head.clone_from(&queued.next);
        }
        if let Some(next) = &queued.next {
            self.cursors
                .get_mut(next)
                .expect("cursor successor must remain indexed")
                .previous
                .clone_from(&queued.previous);
        } else {
            self.tail.clone_from(&queued.previous);
        }
        Some(queued.cursor)
    }

    pub(super) fn remove_cancelled(
        &mut self,
        cancellation: &PreviewCancelFrame,
    ) -> Option<PreviewSendCursor> {
        let cursor = self.cursors.get(&cancellation.stream)?;
        if cursor.cursor.publication().publication_id() > cancellation.publication_id {
            return None;
        }
        self.remove(&cancellation.stream)
    }

    pub(super) fn pop_next(&mut self) -> Option<PreviewSendCursor> {
        let stream = self.head.clone()?;
        let queued = self
            .cursors
            .remove(&stream)
            .expect("cursor head must remain indexed");
        self.state_bytes = self.state_bytes.saturating_sub(preview_cursor_state_bytes(
            queued.cursor.publication().stream(),
        ));
        self.head.clone_from(&queued.next);
        if let Some(next) = &queued.next {
            self.cursors
                .get_mut(next)
                .expect("cursor successor must remain indexed")
                .previous = None;
        } else {
            self.tail = None;
        }
        Some(queued.cursor)
    }

    pub(super) fn requeue(&mut self, cursor: PreviewSendCursor) {
        let stream = cursor.publication().stream().clone();
        debug_assert!(!self.cursors.contains_key(&stream));
        let cursor_bytes = preview_cursor_state_bytes(&stream);
        debug_assert!(self.state_bytes.saturating_add(cursor_bytes) <= self.max_state_bytes);
        self.insert_at_tail(stream, cursor);
        self.state_bytes = self.state_bytes.saturating_add(cursor_bytes);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.cursors.is_empty()
    }
}

fn preview_cursor_state_bytes(stream: &PreviewStreamId) -> usize {
    std::mem::size_of::<QueuedPreviewCursor>()
        .saturating_add(std::mem::size_of::<PreviewStreamId>())
        .saturating_add(stream.identity_bytes().saturating_mul(4))
}

#[derive(Debug, Clone, Copy)]
struct PreviewWireFields {
    frame_number: u32,
    timestamp_ms: u32,
    width: u32,
    height: u32,
    format: WirePreviewFormat,
}

fn validate_preview_fence(
    stream: &PreviewStreamId,
    fence: Option<BrowserInputPublicationId>,
) -> Result<(), PreviewOutboundError> {
    match (stream, fence) {
        (PreviewStreamId::Interactive(_), None) => {
            Err(PreviewOutboundError::MissingInteractiveFence)
        }
        (PreviewStreamId::Interactive(_), Some(_)) | (_, None) => Ok(()),
        (_, Some(_)) => Err(PreviewOutboundError::UnexpectedInteractiveFence),
    }
}

fn decode_preview_fields(
    stream: &PreviewStreamId,
    encoded: &Bytes,
) -> Result<PreviewWireFields, PreviewOutboundError> {
    let invalid = |error: String| PreviewOutboundError::InvalidPublication(error);
    match stream {
        PreviewStreamId::Passive(expected_channel) => {
            let frame = WirePreviewFrame::decode_bytes(encoded)
                .map_err(|error| invalid(error.to_string()))?;
            if frame.channel != *expected_channel {
                return Err(PreviewOutboundError::StreamMismatch);
            }
            Ok(PreviewWireFields::from_preview_frame(&frame))
        }
        PreviewStreamId::Zone { scene_id, zone_id } => {
            let frame = WireZonePreviewFrame::decode_bytes(encoded)
                .map_err(|error| invalid(error.to_string()))?;
            if frame.scene_id != *scene_id || frame.zone_id != *zone_id {
                return Err(PreviewOutboundError::StreamMismatch);
            }
            Ok(PreviewWireFields {
                frame_number: frame.frame_number,
                timestamp_ms: frame.timestamp_ms,
                width: frame.width,
                height: frame.height,
                format: frame.format,
            })
        }
        PreviewStreamId::Display(device_id) => {
            let frame = WireDisplayPreviewFrame::decode_bytes(encoded)
                .map_err(|error| invalid(error.to_string()))?;
            if frame.device_id != *device_id {
                return Err(PreviewOutboundError::StreamMismatch);
            }
            Ok(PreviewWireFields {
                frame_number: frame.frame_number,
                timestamp_ms: frame.timestamp_ms,
                width: frame.width,
                height: frame.height,
                format: frame.format,
            })
        }
        PreviewStreamId::Interactive(expected_preview_id) => {
            let frame = WireInteractivePreviewFrame::decode_bytes(encoded)
                .map_err(|error| invalid(error.to_string()))?;
            if frame.preview_id != *expected_preview_id {
                return Err(PreviewOutboundError::StreamMismatch);
            }
            Ok(PreviewWireFields {
                frame_number: frame.frame_number,
                timestamp_ms: frame.timestamp_ms,
                width: frame.width,
                height: frame.height,
                format: frame.format,
            })
        }
        PreviewStreamId::ScreenZones => {
            let frame = WireScreenZonesFrame::decode(encoded)
                .map_err(|error| invalid(error.to_string()))?;
            Ok(PreviewWireFields {
                frame_number: frame.frame_number,
                timestamp_ms: frame.timestamp_ms,
                width: frame.source_width,
                height: frame.source_height,
                format: WirePreviewFormat::Rgb,
            })
        }
    }
}

impl PreviewWireFields {
    const fn from_preview_frame(frame: &WirePreviewFrame) -> Self {
        Self {
            frame_number: frame.frame_number,
            timestamp_ms: frame.timestamp_ms,
            width: frame.width,
            height: frame.height,
            format: frame.format,
        }
    }
}

fn decoded_preview_bytes(
    stream: &PreviewStreamId,
    width: u32,
    height: u32,
) -> Result<usize, PreviewOutboundError> {
    if matches!(stream, PreviewStreamId::ScreenZones) {
        return Ok(0);
    }
    SurfaceDescriptor::rgba8888(width, height)
        .try_non_empty_byte_len()
        .map_err(|error| PreviewOutboundError::InvalidPublication(error.to_string()))
}

fn set_current_publication(
    current: &mut HashMap<PreviewStreamId, u64>,
    stream: &PreviewStreamId,
    publication_id: u64,
) {
    current.insert(stream.clone(), publication_id);
}

fn remove_current_publication(
    current: &mut HashMap<PreviewStreamId, u64>,
    stream: &PreviewStreamId,
    publication_id: u64,
) {
    if current.get(stream) == Some(&publication_id) {
        current.remove(stream);
    }
}

struct BackpressureReporter {
    pending: Arc<StdMutex<BackpressurePending>>,
    notify: Arc<Notify>,
    task: JoinHandle<()>,
}

#[derive(Debug, Default)]
struct BackpressurePending {
    pending_drops: u32,
    advice: Option<BackpressureAdvice>,
}

#[derive(Debug, Clone, Copy)]
enum BackpressureAdvice {
    ReduceFps(u32),
    ReduceCadence(Cadence),
}

impl BackpressureReporter {
    fn new(
        json_tx: tokio::sync::mpsc::Sender<Utf8Bytes>,
        topic: &'static str,
        key: Option<String>,
    ) -> Self {
        let pending = Arc::new(StdMutex::new(BackpressurePending::default()));
        let notify = Arc::new(Notify::new());
        let task_pending = Arc::clone(&pending);
        let task_notify = Arc::clone(&notify);
        let task = tokio::spawn(async move {
            let mut next_report_at = Instant::now();
            loop {
                task_notify.notified().await;
                tokio::time::sleep(next_report_at.saturating_duration_since(Instant::now())).await;

                let report = {
                    let mut pending = task_pending.lock().unwrap_or_else(PoisonError::into_inner);
                    let advice = pending.advice;
                    let dropped_frames = std::mem::take(&mut pending.pending_drops);
                    advice.map(|advice| (dropped_frames, advice))
                };
                let Some((dropped_frames, advice)) = report else {
                    continue;
                };
                if dropped_frames == 0 {
                    continue;
                }
                if !enqueue_backpressure_notice(
                    &json_tx,
                    topic,
                    key.as_deref(),
                    advice,
                    dropped_frames,
                )
                .await
                {
                    break;
                }

                next_report_at = Instant::now() + BACKPRESSURE_REPORT_INTERVAL;
                debug!(
                    topic,
                    key,
                    dropped_frames,
                    ?advice,
                    "Dropped WebSocket payloads for slow consumer"
                );
                let has_pending = task_pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .pending_drops
                    > 0;
                if has_pending {
                    task_notify.notify_one();
                }
            }
        });

        Self {
            pending,
            notify,
            task,
        }
    }

    fn record_drop(&self, advice: BackpressureAdvice) {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        pending.pending_drops = pending.pending_drops.saturating_add(1);
        pending.advice = Some(advice);
        drop(pending);
        self.notify.notify_one();
    }
}

impl Drop for BackpressureReporter {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(super) async fn publish_preview(
    preview_tx: &PreviewOutboundSender,
    stream: PreviewStreamId,
    payload: Bytes,
    channel: &'static str,
) -> bool {
    loop {
        let capacity_available = preview_tx.shared.capacity_notify.notified();
        tokio::pin!(capacity_available);
        capacity_available.as_mut().enable();
        match preview_tx.publish(stream.clone(), payload.clone(), None) {
            Ok(_) => return true,
            Err(PreviewOutboundError::ConnectionBusy { .. }) => {
                capacity_available.await;
            }
            Err(error) => {
                warn!(channel, %error, "Rejected WebSocket preview publication");
                return false;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreviewRelayPublish {
    Published,
    Rejected,
    SubscriptionChanged,
    SubscriptionsClosed,
}

pub(super) async fn publish_preview_while_subscribed(
    preview_tx: &PreviewOutboundSender,
    stream: PreviewStreamId,
    payload: Bytes,
    channel: &'static str,
    subscriptions: &mut watch::Receiver<SubscriptionState>,
) -> PreviewRelayPublish {
    tokio::select! {
        published = publish_preview(preview_tx, stream, payload, channel) => {
            if published {
                PreviewRelayPublish::Published
            } else {
                PreviewRelayPublish::Rejected
            }
        }
        changed = subscriptions.changed() => {
            if changed.is_err() {
                PreviewRelayPublish::SubscriptionsClosed
            } else {
                let _ = subscriptions.borrow_and_update();
                PreviewRelayPublish::SubscriptionChanged
            }
        }
    }
}

pub(super) async fn publish_preview_until_cancelled(
    preview_tx: &PreviewOutboundSender,
    stream: PreviewStreamId,
    payload: Bytes,
    channel: &'static str,
    cancel: &CancellationToken,
) -> Option<bool> {
    tokio::select! {
        published = publish_preview(preview_tx, stream, payload, channel) => Some(published),
        () = cancel.cancelled() => None,
    }
}

/// Relay discrete events from the broadcast bus to a bounded mpsc channel.
/// Slow consumers backpressure this relay and receive a resync hint after lag.
pub(super) async fn relay_events(
    mut event_rx: broadcast::Receiver<hypercolor_core::bus::TimestampedEvent>,
    json_tx: tokio::sync::mpsc::Sender<Utf8Bytes>,
    subscriptions: watch::Receiver<SubscriptionState>,
) {
    loop {
        match event_rx.recv().await {
            Ok(timestamped) => {
                let should_relay = {
                    let subs = subscriptions.borrow();
                    should_relay_event(&timestamped.event, subs.topics())
                };
                if !should_relay {
                    continue;
                }

                let (event_name, event_data) = event_message_parts(&timestamped.event);
                let msg = ServerMessage::Event {
                    event: event_name,
                    timestamp: timestamped.timestamp.to_string(),
                    data: event_data,
                };
                let Ok(json) = serde_json::to_string(&msg) else {
                    continue;
                };

                if json_tx.send(json.into()).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!("WebSocket consumer lagged by {n} events");
                let should_resync = {
                    let subscriptions = subscriptions.borrow();
                    [TopicId::Events, TopicId::FrameEvents, TopicId::InputEvents]
                        .into_iter()
                        .any(|topic| subscriptions.contains(topic))
                };
                if should_resync {
                    let msg = ServerMessage::Event {
                        event: "resync_required".to_owned(),
                        timestamp: EventTimestamp::now().to_string(),
                        data: serde_json::json!({ "dropped_events": n }),
                    };
                    let Ok(json) = serde_json::to_string(&msg) else {
                        continue;
                    };
                    if json_tx.send(json.into()).await.is_err() {
                        break;
                    }
                }
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Relay frame watch updates to the WebSocket client.
pub(super) async fn relay_frames(
    state: Arc<AppState>,
    json_tx: tokio::sync::mpsc::Sender<Utf8Bytes>,
    binary_tx: tokio::sync::mpsc::Sender<Bytes>,
    mut subscriptions: watch::Receiver<SubscriptionState>,
) {
    let mut frame_rx = None::<watch::Receiver<hypercolor_types::event::FrameData>>;
    let mut active_frame_config = None::<ActiveFramesConfig>;
    let mut last_sent = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);
    let mut was_subscribed = false;
    let backpressure = BackpressureReporter::new(json_tx.clone(), "frames", None);

    loop {
        if active_frame_config.is_none() {
            active_frame_config = {
                let subs = subscriptions.borrow();
                if subs.contains(TopicId::Frames) {
                    Some(ActiveFramesConfig::new(
                        subs.config_of::<FramesConfig>(TopicId::Frames, None),
                    ))
                } else {
                    None
                }
            };
        }
        let Some(frame_config) = active_frame_config.as_ref() else {
            let _ = frame_rx.take();
            was_subscribed = false;
            if subscriptions.changed().await.is_err() {
                break;
            }
            let _ = subscriptions.borrow_and_update();
            continue;
        };
        if frame_rx.is_none() {
            frame_rx = Some(state.event_bus.frame_receiver());
        }
        let frame_rx = frame_rx
            .as_mut()
            .expect("frame receiver should exist while subscribed");

        let emit_current = !was_subscribed;
        was_subscribed = true;
        if !emit_current {
            tokio::select! {
                changed = subscriptions.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let _ = subscriptions.borrow_and_update();
                    active_frame_config = None;
                    continue;
                }
                changed = frame_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                }
            }
        }

        // Clone the frame out of the watch borrow before encoding so the
        // render thread's frame_sender.send_modify() isn't blocked on our
        // serialization. FrameData holds owned Vecs; clone is O(total LEDs).
        let frame = {
            let borrow = frame_rx.borrow();
            if !should_emit(&mut last_sent, frame_config.config.fps) {
                continue;
            }
            borrow.clone()
        };
        let outbound = cached_frame_payload(&frame, frame_config);

        if binary_tx.try_send(outbound).is_err() {
            backpressure.record_drop(BackpressureAdvice::ReduceFps(frame_config.config.fps));
        }
    }
}

/// Relay spectrum watch updates to the WebSocket client.
pub(super) async fn relay_spectrum(
    state: Arc<AppState>,
    json_tx: tokio::sync::mpsc::Sender<Utf8Bytes>,
    binary_tx: tokio::sync::mpsc::Sender<Bytes>,
    mut subscriptions: watch::Receiver<SubscriptionState>,
) {
    let mut spectrum_rx = None::<watch::Receiver<hypercolor_types::event::SpectrumData>>;
    let mut active_spectrum_config = None::<SpectrumConfig>;
    let mut last_sent = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);
    let mut was_subscribed = false;
    let backpressure = BackpressureReporter::new(json_tx.clone(), "spectrum", None);

    loop {
        if active_spectrum_config.is_none() {
            active_spectrum_config = {
                let subs = subscriptions.borrow();
                if subs.contains(TopicId::Spectrum) {
                    Some(subs.config_of::<SpectrumConfig>(TopicId::Spectrum, None))
                } else {
                    None
                }
            };
        }
        let Some(spectrum_config) = active_spectrum_config.as_ref() else {
            let _ = spectrum_rx.take();
            was_subscribed = false;
            if subscriptions.changed().await.is_err() {
                break;
            }
            let _ = subscriptions.borrow_and_update();
            continue;
        };
        if spectrum_rx.is_none() {
            spectrum_rx = Some(state.event_bus.spectrum_receiver());
        }
        let spectrum_rx = spectrum_rx
            .as_mut()
            .expect("spectrum receiver should exist while subscribed");

        let emit_current = !was_subscribed;
        was_subscribed = true;
        if !emit_current {
            tokio::select! {
                changed = subscriptions.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let _ = subscriptions.borrow_and_update();
                    active_spectrum_config = None;
                    continue;
                }
                changed = spectrum_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                }
            }
        }

        // Mirror the frame/canvas relays: drop the watch borrow before
        // encoding so the render thread's spectrum_sender.send_modify()
        // isn't blocked on our serialization.
        let spectrum = {
            let borrow = spectrum_rx.borrow();
            if !should_emit(&mut last_sent, spectrum_config.fps) {
                continue;
            }
            borrow.clone()
        };
        if binary_tx
            .try_send(cached_spectrum_payload(&spectrum, spectrum_config.bins))
            .is_err()
        {
            backpressure.record_drop(BackpressureAdvice::ReduceFps(spectrum_config.fps));
        }
    }
}

/// Relay raw canvas updates to the WebSocket client.
#[expect(
    clippy::too_many_lines,
    reason = "canvas relay interleaves subscription, power, tick, and cache state in one async loop"
)]
pub(super) async fn relay_canvas(
    preview_runtime: Arc<crate::preview_runtime::PreviewRuntime>,
    mut power_state_rx: watch::Receiver<OutputPowerState>,
    preview_tx: PreviewOutboundSender,
    mut subscriptions: watch::Receiver<SubscriptionState>,
) {
    let mut canvas_rx = None::<crate::preview_runtime::PreviewFrameReceiver>;
    let mut active_canvas_config = None::<CanvasConfig>;
    let mut receiver_initialized = false;
    let mut last_sent_surface = None::<PreviewSurfaceIdentity>;
    let mut pending_send = false;
    let mut active_fps = 15_u32;
    let mut last_sent_at = preview_initial_last_sent();

    'relay: loop {
        if active_canvas_config.is_none() {
            active_canvas_config = {
                let subs = subscriptions.borrow();
                if subs.contains(TopicId::Canvas) {
                    Some(subs.config_of::<CanvasConfig>(TopicId::Canvas, None))
                } else {
                    None
                }
            };
        }
        sync_preview_receiver(&mut canvas_rx, active_canvas_config.is_some(), || {
            preview_runtime.canvas_receiver()
        });

        let Some(canvas_config) = active_canvas_config.as_ref() else {
            last_sent_surface = None;
            receiver_initialized = false;
            pending_send = false;
            last_sent_at = preview_initial_last_sent();
            tokio::select! {
                changed = power_state_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let _ = power_state_rx.borrow_and_update();
                }
                changed = subscriptions.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let _ = subscriptions.borrow_and_update();
                    active_canvas_config = None;
                }
            }
            continue;
        };
        let canvas_rx = canvas_rx
            .as_mut()
            .expect("preview canvas receiver should exist while subscribed");
        canvas_rx.update_demand(preview_stream_demand(canvas_config));

        if canvas_config.fps != active_fps {
            active_fps = canvas_config.fps.max(1);
        }
        if !receiver_initialized {
            let _ = canvas_rx.borrow_and_update();
            receiver_initialized = true;
            pending_send = true;
        }

        tokio::select! {
            changed = canvas_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let _ = canvas_rx.borrow_and_update();
                pending_send = true;
            }
            changed = power_state_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let _ = power_state_rx.borrow_and_update();
                pending_send |= receiver_initialized;
            }
            changed = subscriptions.changed() => {
                if changed.is_err() {
                    break;
                }
                let _ = subscriptions.borrow_and_update();
                active_canvas_config = None;
            }
            () = tokio::time::sleep(preview_send_delay(last_sent_at, active_fps, Instant::now())), if pending_send => {
                // Clone out of the watch borrow before encoding so the
                // render thread's canvas_sender().send() isn't blocked on
                // bilinear/JPEG work. CanvasFrame's pixel storage is
                // Arc-backed, so clone is cheap (refcount bumps).
                let (canvas_snapshot, surface_identity) = {
                    let latest_canvas = canvas_rx.borrow();
                    let surface_identity = preview_surface_identity(&latest_canvas);
                    if last_sent_surface == Some(surface_identity) {
                        pending_send = false;
                        continue;
                    }
                    (latest_canvas.clone(), surface_identity)
                };

                // Preview always renders at full brightness — the brightness
                // slider affects device output, not the UI canvas preview.
                let payload = try_encode_cached_canvas_preview_binary(
                    &canvas_snapshot,
                    canvas_config.format,
                    1.0,
                    canvas_config.width,
                    canvas_config.height,
                );

                let Some(payload) = payload else {
                    pending_send = false;
                    continue;
                };

                match publish_preview_while_subscribed(
                    &preview_tx,
                    PreviewStreamId::Passive(PreviewFrameChannel::Canvas),
                    payload,
                    "canvas",
                    &mut subscriptions,
                ).await {
                    PreviewRelayPublish::Published => {}
                    PreviewRelayPublish::Rejected => {
                        last_sent_at = Instant::now();
                        pending_send = false;
                        continue;
                    }
                    PreviewRelayPublish::SubscriptionChanged => {
                        active_canvas_config = None;
                        continue 'relay;
                    }
                    PreviewRelayPublish::SubscriptionsClosed => break 'relay,
                }

                last_sent_at = Instant::now();
                last_sent_surface = Some(surface_identity);
                pending_send = false;
            }
        }
    }
}

/// Relay raw screen-source canvas updates to the WebSocket client.
pub(super) async fn relay_screen_canvas(
    preview_runtime: Arc<crate::preview_runtime::PreviewRuntime>,
    preview_tx: PreviewOutboundSender,
    mut subscriptions: watch::Receiver<SubscriptionState>,
) {
    let mut canvas_rx = None::<crate::preview_runtime::PreviewFrameReceiver>;
    let mut active_canvas_config = None::<CanvasConfig>;
    let mut receiver_initialized = false;
    let mut last_sent_surface = None::<PreviewSurfaceIdentity>;
    let mut pending_send = false;
    let mut active_fps = 15_u32;
    let mut last_sent_at = preview_initial_last_sent();

    'relay: loop {
        if active_canvas_config.is_none() {
            active_canvas_config = {
                let subs = subscriptions.borrow();
                if subs.contains(TopicId::ScreenCanvas) {
                    Some(subs.config_of::<CanvasConfig>(TopicId::ScreenCanvas, None))
                } else {
                    None
                }
            };
        }
        sync_preview_receiver(&mut canvas_rx, active_canvas_config.is_some(), || {
            preview_runtime.screen_canvas_receiver()
        });

        let Some(canvas_config) = active_canvas_config.as_ref() else {
            last_sent_surface = None;
            receiver_initialized = false;
            pending_send = false;
            last_sent_at = preview_initial_last_sent();
            if subscriptions.changed().await.is_err() {
                break;
            }
            let _ = subscriptions.borrow_and_update();
            active_canvas_config = None;
            continue;
        };
        let canvas_rx = canvas_rx
            .as_mut()
            .expect("screen preview receiver should exist while subscribed");
        canvas_rx.update_demand(preview_stream_demand(canvas_config));

        if canvas_config.fps != active_fps {
            active_fps = canvas_config.fps.max(1);
        }
        if !receiver_initialized {
            let _ = canvas_rx.borrow_and_update();
            receiver_initialized = true;
            pending_send = true;
        }

        tokio::select! {
            changed = canvas_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let _ = canvas_rx.borrow_and_update();
                pending_send = true;
            }
            changed = subscriptions.changed() => {
                if changed.is_err() {
                    break;
                }
                let _ = subscriptions.borrow_and_update();
                active_canvas_config = None;
            }
            () = tokio::time::sleep(preview_send_delay(last_sent_at, active_fps, Instant::now())), if pending_send => {
                // See relay_canvas for why we clone out of the borrow before
                // encoding — avoids blocking the render thread's watch writer.
                let (canvas_snapshot, surface_identity) = {
                    let latest_canvas = canvas_rx.borrow();
                    let surface_identity = preview_surface_identity(&latest_canvas);
                    if last_sent_surface == Some(surface_identity) {
                        pending_send = false;
                        continue;
                    }
                    (latest_canvas.clone(), surface_identity)
                };

                let payload = try_encode_cached_canvas_binary_with_header_scaled(
                    &canvas_snapshot,
                    canvas_config.format,
                    WS_SCREEN_CANVAS_HEADER,
                    canvas_config.width,
                    canvas_config.height,
                );

                let Some(payload) = payload else {
                    pending_send = false;
                    continue;
                };

                match publish_preview_while_subscribed(
                    &preview_tx,
                    PreviewStreamId::Passive(PreviewFrameChannel::ScreenCanvas),
                    payload,
                    "screen_canvas",
                    &mut subscriptions,
                ).await {
                    PreviewRelayPublish::Published => {}
                    PreviewRelayPublish::Rejected => {
                        last_sent_at = Instant::now();
                        pending_send = false;
                        continue;
                    }
                    PreviewRelayPublish::SubscriptionChanged => {
                        active_canvas_config = None;
                        continue 'relay;
                    }
                    PreviewRelayPublish::SubscriptionsClosed => break 'relay,
                }

                last_sent_at = Instant::now();
                last_sent_surface = Some(surface_identity);
                pending_send = false;
            }
        }
    }
}

/// Relay ambilight zone-grid frames to a subscribed client.
///
/// Zone frames keep their source dimensions and RGB payload, but each
/// connection owns its publication cadence. Watch semantics coalesce source
/// updates while the configured interval is still running.
pub(super) async fn relay_screen_zones(
    preview_runtime: Arc<crate::preview_runtime::PreviewRuntime>,
    mut subscriptions: watch::Receiver<SubscriptionState>,
    preview_tx: PreviewOutboundSender,
) {
    let mut zones_rx = None::<tokio::sync::watch::Receiver<hypercolor_core::bus::ScreenZonesFrame>>;
    let mut active_config = None::<ScreenZonesConfig>;
    let mut receiver_initialized = false;
    let mut pending_send = false;
    let mut last_sent_at = preview_initial_last_sent();

    'relay: loop {
        if active_config.is_none() {
            active_config = {
                let subscriptions = subscriptions.borrow();
                subscriptions
                    .contains(TopicId::ScreenZones)
                    .then(|| subscriptions.config_of(TopicId::ScreenZones, None))
            };
        }
        let subscribed = active_config.is_some();
        if subscribed && zones_rx.is_none() {
            let mut receiver = preview_runtime.screen_zones_receiver();
            receiver.mark_changed();
            zones_rx = Some(receiver);
        } else if !subscribed {
            zones_rx = None;
        }

        let Some(ref config) = active_config else {
            receiver_initialized = false;
            pending_send = false;
            last_sent_at = preview_initial_last_sent();
            if subscriptions.changed().await.is_err() {
                break;
            }
            let _ = subscriptions.borrow_and_update();
            active_config = None;
            continue;
        };
        let receiver = zones_rx
            .as_mut()
            .expect("screen zones receiver should exist while subscribed");
        if !receiver_initialized {
            let _ = receiver.borrow_and_update();
            receiver_initialized = true;
            pending_send = true;
        }

        tokio::select! {
            changed = receiver.changed() => {
                if changed.is_err() {
                    break;
                }
                let _ = receiver.borrow_and_update();
                pending_send = true;
            }
            changed = subscriptions.changed() => {
                if changed.is_err() {
                    break;
                }
                let _ = subscriptions.borrow_and_update();
                active_config = None;
            }
            () = tokio::time::sleep(
                preview_send_delay(last_sent_at, config.fps.max(1), Instant::now())
            ), if pending_send => {
                let frame = receiver.borrow().clone();
                let payload = match encode_screen_zones_frame(&frame) {
                    Ok(payload) => payload,
                    Err(error) => {
                        warn!(%error, "Failed to encode screen zones preview");
                        pending_send = false;
                        continue;
                    }
                };
                match publish_preview_while_subscribed(
                    &preview_tx,
                    PreviewStreamId::ScreenZones,
                    payload,
                    "screen_zones",
                    &mut subscriptions,
                ).await {
                    PreviewRelayPublish::Published => {
                        last_sent_at = Instant::now();
                    }
                    PreviewRelayPublish::Rejected => {}
                    PreviewRelayPublish::SubscriptionChanged => {
                        active_config = None;
                        continue 'relay;
                    }
                    PreviewRelayPublish::SubscriptionsClosed => break 'relay,
                }
                pending_send = false;
            }
        }
    }
}

pub(super) fn encode_screen_zones_frame(
    frame: &hypercolor_core::bus::ScreenZonesFrame,
) -> Result<Bytes, PreviewOutboundError> {
    let payload_len = frame.colors.len().checked_mul(3).ok_or_else(|| {
        PreviewOutboundError::InvalidPublication(
            "screen zones payload length exceeds address space".to_owned(),
        )
    })?;
    let mut payload = Vec::new();
    payload.try_reserve_exact(payload_len).map_err(|_| {
        PreviewOutboundError::InvalidPublication(format!(
            "screen zones payload allocation failed for {payload_len} bytes"
        ))
    })?;
    for color in frame.colors.iter() {
        payload.extend_from_slice(color);
    }

    hypercolor_leptos_ext::ws::ScreenZonesFrame {
        frame_number: frame.frame_number,
        timestamp_ms: frame.timestamp_ms,
        source_width: frame.source_width,
        source_height: frame.source_height,
        grid_cols: frame.grid_cols,
        grid_rows: frame.grid_rows,
        letterbox: frame.letterbox,
        payload: Bytes::from(payload),
    }
    .try_encode()
    .map_err(|error| PreviewOutboundError::InvalidPublication(error.to_string()))
}

pub(super) async fn relay_web_viewport_canvas(
    preview_runtime: Arc<crate::preview_runtime::PreviewRuntime>,
    preview_tx: PreviewOutboundSender,
    mut subscriptions: watch::Receiver<SubscriptionState>,
) {
    let mut canvas_rx = None::<crate::preview_runtime::PreviewFrameReceiver>;
    let mut active_canvas_config = None::<CanvasConfig>;
    let mut receiver_initialized = false;
    let mut last_sent_surface = None::<PreviewSurfaceIdentity>;
    let mut pending_send = false;
    let mut active_fps = 15_u32;
    let mut last_sent_at = preview_initial_last_sent();

    'relay: loop {
        if active_canvas_config.is_none() {
            active_canvas_config = {
                let subs = subscriptions.borrow();
                if subs.contains(TopicId::WebViewportCanvas) {
                    Some(subs.config_of::<CanvasConfig>(TopicId::WebViewportCanvas, None))
                } else {
                    None
                }
            };
        }
        sync_preview_receiver(&mut canvas_rx, active_canvas_config.is_some(), || {
            preview_runtime.web_viewport_canvas_receiver()
        });

        let Some(canvas_config) = active_canvas_config.as_ref() else {
            last_sent_surface = None;
            receiver_initialized = false;
            pending_send = false;
            last_sent_at = preview_initial_last_sent();
            if subscriptions.changed().await.is_err() {
                break;
            }
            let _ = subscriptions.borrow_and_update();
            active_canvas_config = None;
            continue;
        };
        let canvas_rx = canvas_rx
            .as_mut()
            .expect("web viewport preview receiver should exist while subscribed");
        canvas_rx.update_demand(preview_stream_demand(canvas_config));

        if canvas_config.fps != active_fps {
            active_fps = canvas_config.fps.max(1);
        }
        if !receiver_initialized {
            let _ = canvas_rx.borrow_and_update();
            receiver_initialized = true;
            pending_send = true;
        }

        tokio::select! {
            changed = canvas_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let _ = canvas_rx.borrow_and_update();
                pending_send = true;
            }
            changed = subscriptions.changed() => {
                if changed.is_err() {
                    break;
                }
                let _ = subscriptions.borrow_and_update();
                active_canvas_config = None;
            }
            () = tokio::time::sleep(preview_send_delay(last_sent_at, active_fps, Instant::now())), if pending_send => {
                // See relay_canvas for why we clone out of the borrow before
                // encoding — avoids blocking the render thread's watch writer.
                let (canvas_snapshot, surface_identity) = {
                    let latest_canvas = canvas_rx.borrow();
                    let surface_identity = preview_surface_identity(&latest_canvas);
                    if last_sent_surface == Some(surface_identity) {
                        pending_send = false;
                        continue;
                    }
                    (latest_canvas.clone(), surface_identity)
                };

                let payload = try_encode_cached_canvas_binary_with_header_scaled(
                    &canvas_snapshot,
                    canvas_config.format,
                    WS_WEB_VIEWPORT_CANVAS_HEADER,
                    canvas_config.width,
                    canvas_config.height,
                );

                let Some(payload) = payload else {
                    pending_send = false;
                    continue;
                };

                match publish_preview_while_subscribed(
                    &preview_tx,
                    PreviewStreamId::Passive(PreviewFrameChannel::WebViewportCanvas),
                    payload,
                    "web_viewport_canvas",
                    &mut subscriptions,
                ).await {
                    PreviewRelayPublish::Published => {}
                    PreviewRelayPublish::Rejected => {
                        last_sent_at = Instant::now();
                        pending_send = false;
                        continue;
                    }
                    PreviewRelayPublish::SubscriptionChanged => {
                        active_canvas_config = None;
                        continue 'relay;
                    }
                    PreviewRelayPublish::SubscriptionsClosed => break 'relay,
                }

                last_sent_at = Instant::now();
                last_sent_surface = Some(surface_identity);
                pending_send = false;
            }
        }
    }
}

pub(super) async fn relay_zone_preview(
    preview_runtime: Arc<crate::preview_runtime::PreviewRuntime>,
    preview_tx: PreviewOutboundSender,
    mut subscriptions: watch::Receiver<SubscriptionState>,
) {
    let mut preview_rx = None::<crate::preview_runtime::ZonePreviewFrameReceiver>;
    let mut active_canvas_config = None::<CanvasConfig>;
    let mut receiver_initialized = false;
    let mut last_sent_surfaces = HashMap::<PreviewStreamId, PreviewSurfaceIdentity>::new();
    let mut pending_send = false;
    let mut active_fps = 15_u32;
    let mut last_sent_at = preview_initial_last_sent();

    'relay: loop {
        if active_canvas_config.is_none() {
            active_canvas_config = {
                let subs = subscriptions.borrow();
                if subs.contains(TopicId::ZonePreview) {
                    Some(subs.config_of::<CanvasConfig>(TopicId::ZonePreview, None))
                } else {
                    None
                }
            };
        }
        sync_zone_preview_receiver(&mut preview_rx, active_canvas_config.is_some(), || {
            preview_runtime.zone_preview_receiver()
        });

        let Some(canvas_config) = active_canvas_config.as_ref() else {
            last_sent_surfaces.clear();
            receiver_initialized = false;
            pending_send = false;
            last_sent_at = preview_initial_last_sent();
            if subscriptions.changed().await.is_err() {
                break;
            }
            let _ = subscriptions.borrow_and_update();
            active_canvas_config = None;
            continue;
        };
        let preview_rx = preview_rx
            .as_mut()
            .expect("zone preview receiver should exist while subscribed");
        preview_rx.update_demand(preview_stream_demand(canvas_config));

        if canvas_config.fps != active_fps {
            active_fps = canvas_config.fps.max(1);
        }
        if !receiver_initialized {
            let _ = preview_rx.borrow_and_update();
            receiver_initialized = true;
            pending_send = true;
        }

        tokio::select! {
            changed = preview_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let _ = preview_rx.borrow_and_update();
                pending_send = true;
            }
            changed = subscriptions.changed() => {
                if changed.is_err() {
                    break;
                }
                let _ = subscriptions.borrow_and_update();
                active_canvas_config = None;
            }
            () = tokio::time::sleep(preview_send_delay(last_sent_at, active_fps, Instant::now())), if pending_send => {
                let zone_previews = {
                    let latest = preview_rx.borrow();
                    latest.clone()
                };
                let mut active_streams = HashSet::new();
                for zone_preview in &zone_previews {
                    let stream = PreviewStreamId::Zone {
                        scene_id: *zone_preview.scene_id.0.as_bytes(),
                        zone_id: *zone_preview.zone_id.0.as_bytes(),
                    };
                    active_streams.insert(stream.clone());
                    let surface_identity = preview_surface_identity(&zone_preview.frame);
                    if last_sent_surfaces.get(&stream) == Some(&surface_identity) {
                        continue;
                    }
                    let payload = try_encode_cached_zone_preview_binary_scaled(
                        zone_preview,
                        canvas_config.format,
                        canvas_config.width,
                        canvas_config.height,
                    );
                    let Some(payload) = payload else {
                        continue;
                    };
                    match publish_preview_while_subscribed(
                        &preview_tx,
                        stream.clone(),
                        payload,
                        "zone_preview",
                        &mut subscriptions,
                    ).await {
                        PreviewRelayPublish::Published => {}
                        PreviewRelayPublish::Rejected => continue,
                        PreviewRelayPublish::SubscriptionChanged => {
                            active_canvas_config = None;
                            continue 'relay;
                        }
                        PreviewRelayPublish::SubscriptionsClosed => break 'relay,
                    }
                    last_sent_surfaces.insert(stream, surface_identity);
                }
                let retired = last_sent_surfaces
                    .keys()
                    .filter(|stream| !active_streams.contains(*stream))
                    .cloned()
                    .collect::<Vec<_>>();
                for stream in retired {
                    match preview_tx.cancel(&stream) {
                        Ok(_) => {
                            last_sent_surfaces.remove(&stream);
                        }
                        Err(error) => {
                            warn!(%error, "Failed to cancel retired zone preview stream");
                        }
                    }
                }
                last_sent_at = Instant::now();
                pending_send = false;
            }
        }
    }
}

pub(super) fn sync_preview_receiver(
    receiver: &mut Option<crate::preview_runtime::PreviewFrameReceiver>,
    subscribed: bool,
    subscribe: impl FnOnce() -> crate::preview_runtime::PreviewFrameReceiver,
) {
    if subscribed {
        if receiver.is_none() {
            *receiver = Some(subscribe());
        }
    } else {
        let _ = receiver.take();
    }
}

fn sync_zone_preview_receiver(
    receiver: &mut Option<crate::preview_runtime::ZonePreviewFrameReceiver>,
    subscribed: bool,
    subscribe: impl FnOnce() -> crate::preview_runtime::ZonePreviewFrameReceiver,
) {
    if subscribed {
        if receiver.is_none() {
            *receiver = Some(subscribe());
        }
    } else {
        let _ = receiver.take();
    }
}

/// One followed display: a per-key task plus the handles that steer it.
struct DisplayPreviewFollower {
    cadence: watch::Sender<u32>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

/// Supervise one relay task per live `display_preview` key.
///
/// `display_preview` is keyed by device, so a connection can follow
/// several displays at once. A task per key keeps each display's pacing
/// independent — a 30fps panel cannot starve a 2fps one — and each frame
/// names its device in the header, so the client routes them without
/// guessing from resolution.
pub(super) async fn relay_display_preview(
    state: Arc<AppState>,
    display_frames: Arc<tokio::sync::RwLock<crate::display_frames::DisplayFrameRuntime>>,
    preview_tx: PreviewOutboundSender,
    mut subscriptions: watch::Receiver<SubscriptionState>,
) {
    use hypercolor_types::device::DeviceId;
    use std::str::FromStr;

    let mut followers: HashMap<String, DisplayPreviewFollower> = HashMap::new();
    let (completed_tx, mut completed_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let mut device_events = state.event_bus.subscribe_all();

    loop {
        // A key naming a device this daemon cannot preview is dropped
        // rather than refused: the subscription is legitimate, there is
        // simply nothing to send until such a device appears.
        let desired: Vec<(String, DeviceId, u32)> = {
            let subs = subscriptions.borrow();
            subs.keyed_configs::<DisplayPreviewConfig>(TopicId::DisplayPreview)
                .into_iter()
                .filter_map(|(key, config)| {
                    let device_id = DeviceId::from_str(&key).ok()?;
                    Some((key, device_id, config.fps.max(1)))
                })
                .collect()
        };

        let retired: Vec<String> = followers
            .keys()
            .filter(|key| !desired.iter().any(|(wanted, _, _)| wanted == *key))
            .cloned()
            .collect();
        for key in retired {
            if let Some(follower) = followers.remove(&key) {
                follower.cancel.cancel();
                let _ = follower.task.await;
            }
        }

        for (wire_key, device_id, fps) in desired {
            if let Some(follower) = followers.get(&wire_key) {
                let _ = follower.cadence.send(fps);
                continue;
            }
            let known_display_device =
                state
                    .device_registry
                    .get(&device_id)
                    .await
                    .is_some_and(|tracked| {
                        crate::api::displays::display_surface_info(&tracked.info).is_some()
                    });
            if !known_display_device {
                continue;
            }
            let (cadence, cadence_rx) = watch::channel(fps);
            let cancel = CancellationToken::new();
            let follower_key = wire_key.clone();
            let follower_completed = completed_tx.clone();
            let follower_state = Arc::clone(&state);
            let follower_frames = Arc::clone(&display_frames);
            let follower_preview_tx = preview_tx.clone();
            let follower_cancel = cancel.clone();
            let task = tokio::spawn(async move {
                follow_display_preview(
                    follower_state,
                    follower_frames,
                    device_id,
                    follower_key.clone(),
                    cadence_rx,
                    follower_preview_tx,
                    follower_cancel,
                )
                .await;
                let _ = follower_completed.send(follower_key);
            });
            drop(followers.insert(
                wire_key,
                DisplayPreviewFollower {
                    cadence,
                    cancel,
                    task,
                },
            ));
        }

        tokio::select! {
            changed = subscriptions.changed() => {
                if changed.is_err() {
                    break;
                }
                let _ = subscriptions.borrow_and_update();
            }
            completed = completed_rx.recv() => {
                let Some(completed) = completed else {
                    break;
                };
                if let Some(follower) = followers.remove(&completed) {
                    let _ = follower.task.await;
                }
            }
            live = wait_for_display_device_change(&mut device_events) => {
                if !live {
                    break;
                }
            }
        }
    }

    for (_, follower) in followers {
        follower.cancel.cancel();
        let _ = follower.task.await;
    }
}

async fn wait_for_display_device_change(
    events: &mut broadcast::Receiver<hypercolor_core::bus::TimestampedEvent>,
) -> bool {
    loop {
        match events.recv().await {
            Ok(timestamped)
                if matches!(
                    timestamped.event,
                    HypercolorEvent::DeviceConnected { .. }
                        | HypercolorEvent::DeviceDisconnected { .. }
                        | HypercolorEvent::DeviceStateChanged { .. }
                ) =>
            {
                return true;
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(_)) => return true,
            Err(broadcast::error::RecvError::Closed) => return false,
        }
    }
}

/// Stream one display's frames until the subscription retires.
///
/// A closed watch sender is a display-worker rebuild, not device removal,
/// so the task resubscribes as long as the registry still knows the
/// device; once it does not, the task ends and the supervisor rebuilds it
/// when the client's subscriptions next change.
async fn follow_display_preview(
    state: Arc<AppState>,
    display_frames: Arc<tokio::sync::RwLock<crate::display_frames::DisplayFrameRuntime>>,
    device_id: hypercolor_types::device::DeviceId,
    wire_key: String,
    mut cadence: watch::Receiver<u32>,
    preview_tx: PreviewOutboundSender,
    cancel: CancellationToken,
) {
    let mut last_frame_number: Option<u64> = None;
    let mut last_sent_at = preview_initial_last_sent();

    'attach: loop {
        let mut frames = display_frames.write().await.subscribe(device_id);
        // `watch::Sender::subscribe()` marks the new receiver as
        // already-observed, so `changed()` will not fire for the initial
        // value. Prime the send when a snapshot already exists, or the
        // client would stall until the daemon publishes a fresh frame.
        let mut pending_send = frames.borrow().is_some();

        loop {
            let fps = (*cadence.borrow()).max(1);
            tokio::select! {
                () = cancel.cancelled() => return,
                changed = frames.changed() => {
                    if changed.is_err() {
                        if let Err(error) = preview_tx.cancel(&PreviewStreamId::Display(wire_key.clone())) {
                            warn!(%error, device_id = %device_id, "Failed to cancel closed display preview stream");
                        }
                        break;
                    }
                    // Either a new frame or the terminal None marker;
                    // the send path decides which.
                    pending_send = true;
                }
                changed = cadence.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
                () = tokio::time::sleep(preview_send_delay(last_sent_at, fps, Instant::now())), if pending_send => {
                    pending_send = false;
                    let Some(snapshot) = frames.borrow().as_ref().map(Arc::clone) else {
                        if let Err(error) = preview_tx.cancel(&PreviewStreamId::Display(wire_key.clone())) {
                            warn!(%error, device_id = %device_id, "Failed to cancel retired display preview stream");
                        }
                        break;
                    };
                    if last_frame_number == Some(snapshot.frame_number) {
                        // No forward motion since the last send.
                        continue;
                    }
                    let Some(payload) = cached_display_preview_payload(device_id, &snapshot) else {
                        last_sent_at = Instant::now();
                        continue;
                    };
                    let Some(published) = publish_preview_until_cancelled(
                        &preview_tx,
                        PreviewStreamId::Display(wire_key.clone()),
                        payload,
                        "display_preview",
                        &cancel,
                    ).await else {
                        return;
                    };
                    if published {
                        last_frame_number = Some(snapshot.frame_number);
                    }
                    // Either way the clock advances, so a rejected
                    // publication waits out a full interval instead of
                    // spinning the encoder.
                    last_sent_at = Instant::now();
                }
            }
        }

        let still_previewable =
            state
                .device_registry
                .get(&device_id)
                .await
                .is_some_and(|tracked| {
                    crate::api::displays::display_surface_info(&tracked.info).is_some()
                });
        if !still_previewable {
            return;
        }
        continue 'attach;
    }
}

fn preview_stream_demand(config: &CanvasConfig) -> PreviewStreamDemand {
    PreviewStreamDemand {
        fps: config.fps,
        format: match config.format {
            CanvasFormat::Rgb => PreviewPixelFormat::Rgb,
            CanvasFormat::Rgba => PreviewPixelFormat::Rgba,
            CanvasFormat::Jpeg => PreviewPixelFormat::Jpeg,
        },
        width: config.width,
        height: config.height,
    }
}

fn metrics_preview_demand(summary: PreviewDemandSummary) -> MetricsPreviewDemand {
    MetricsPreviewDemand {
        subscribers: summary.subscribers,
        max_fps: summary.max_fps,
        max_width: summary.max_width,
        max_height: summary.max_height,
        any_full_resolution: summary.any_full_resolution,
        any_rgb: summary.any_rgb,
        any_rgba: summary.any_rgba,
        any_jpeg: summary.any_jpeg,
    }
}

/// Relay periodic metrics snapshots to the WebSocket client.
pub(super) async fn relay_metrics(
    state: Arc<AppState>,
    json_tx: tokio::sync::mpsc::Sender<Utf8Bytes>,
    mut subscriptions: watch::Receiver<SubscriptionState>,
) {
    let mut last_total_bytes = WS_TOTAL_BYTES_SENT.load(Ordering::Relaxed);
    let mut active_cadence = None::<Cadence>;
    let backpressure = BackpressureReporter::new(json_tx.clone(), "metrics", None);

    loop {
        if active_cadence.is_none() {
            active_cadence = {
                let subs = subscriptions.borrow();
                if subs.contains(TopicId::Metrics) {
                    Some(subs.config_of::<MetricsConfig>(TopicId::Metrics, None).fps)
                } else {
                    None
                }
            };
        }

        let Some(cadence) = active_cadence else {
            if subscriptions.changed().await.is_err() {
                break;
            }
            let _ = subscriptions.borrow_and_update();
            continue;
        };
        tokio::select! {
            changed = subscriptions.changed() => {
                if changed.is_err() {
                    break;
                }
                let _ = subscriptions.borrow_and_update();
                active_cadence = None;
                continue;
            }
            () = tokio::time::sleep(cadence.period()) => {}
        }

        let still_subscribed = {
            let subs = subscriptions.borrow();
            subs.contains(TopicId::Metrics)
        };
        if !still_subscribed {
            continue;
        }

        let total_bytes = WS_TOTAL_BYTES_SENT.load(Ordering::Relaxed);
        let delta_bytes = total_bytes.saturating_sub(last_total_bytes);
        last_total_bytes = total_bytes;
        let delta_u32 = u32::try_from(delta_bytes).unwrap_or(u32::MAX);
        let bytes_per_sec = f64::from(delta_u32) * cadence.fps();

        let message = build_metrics_message(&state, bytes_per_sec).await;
        if let Ok(text) = serde_json::to_string(&message)
            && !try_enqueue_json(&json_tx, text, "metrics")
        {
            let suggested_fps = (cadence.fps() / 2.0).max(METRICS_FPS_MIN);
            let suggested = Cadence::from_fps(suggested_fps).unwrap_or_default();
            backpressure.record_drop(BackpressureAdvice::ReduceCadence(suggested));
        }
    }
}

/// Relay periodic per-device metrics snapshots to the WebSocket client.
pub(super) async fn relay_device_metrics(
    state: Arc<AppState>,
    json_tx: tokio::sync::mpsc::Sender<Utf8Bytes>,
    mut subscriptions: watch::Receiver<SubscriptionState>,
) {
    let mut active_cadence = None::<Cadence>;
    let backpressure = BackpressureReporter::new(json_tx.clone(), "device_metrics", None);

    loop {
        if active_cadence.is_none() {
            active_cadence = {
                let subs = subscriptions.borrow();
                if subs.contains(TopicId::DeviceMetrics) {
                    Some(
                        subs.config_of::<MetricsConfig>(TopicId::DeviceMetrics, None)
                            .fps,
                    )
                } else {
                    None
                }
            };
        }

        let Some(cadence) = active_cadence else {
            if subscriptions.changed().await.is_err() {
                break;
            }
            let _ = subscriptions.borrow_and_update();
            continue;
        };
        tokio::select! {
            changed = subscriptions.changed() => {
                if changed.is_err() {
                    break;
                }
                let _ = subscriptions.borrow_and_update();
                active_cadence = None;
                continue;
            }
            () = tokio::time::sleep(cadence.period()) => {}
        }

        let still_subscribed = {
            let subs = subscriptions.borrow();
            subs.contains(TopicId::DeviceMetrics)
        };
        if !still_subscribed {
            continue;
        }

        let message = build_device_metrics_message(&state);
        if let Ok(text) = serde_json::to_string(&message)
            && !try_enqueue_json(&json_tx, text, "device_metrics")
        {
            let suggested_fps = (cadence.fps() / 2.0).max(METRICS_FPS_MIN);
            let suggested = Cadence::from_fps(suggested_fps).unwrap_or_default();
            backpressure.record_drop(BackpressureAdvice::ReduceCadence(suggested));
        }
    }
}

/// Relay latest-value system sensor snapshots to the WebSocket client.
pub(super) async fn relay_sensors(
    state: Arc<AppState>,
    json_tx: tokio::sync::mpsc::Sender<Utf8Bytes>,
    mut subscriptions: watch::Receiver<SubscriptionState>,
) {
    let mut sensor_rx = sensor_snapshot_receiver(&state).await;
    let mut sent_current_snapshot = false;

    loop {
        if !subscriptions.borrow().contains(TopicId::Sensors) {
            sent_current_snapshot = false;
            if subscriptions.changed().await.is_err() {
                break;
            }
            let _ = subscriptions.borrow_and_update();
            continue;
        }

        let Some(rx) = sensor_rx.as_mut() else {
            if !sent_current_snapshot {
                tokio::select! {
                    changed = subscriptions.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let _ = subscriptions.borrow_and_update();
                    }
                    permit = json_tx.reserve() => {
                        let Ok(permit) = permit else {
                            break;
                        };
                        if subscriptions.borrow().contains(TopicId::Sensors)
                            && let Some(message) = sensor_snapshot_message(&SystemSnapshot::empty())
                        {
                            permit.send(message);
                            sent_current_snapshot = true;
                        }
                    }
                }
                continue;
            }

            if subscriptions.changed().await.is_err() {
                break;
            }
            let _ = subscriptions.borrow_and_update();
            sensor_rx = sensor_snapshot_receiver(&state).await;
            continue;
        };

        if sent_current_snapshot {
            tokio::select! {
                changed = subscriptions.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let _ = subscriptions.borrow_and_update();
                }
                changed = rx.changed() => {
                    if changed.is_err() {
                        sensor_rx = sensor_snapshot_receiver(&state).await;
                    }
                    sent_current_snapshot = false;
                }
            }
            continue;
        }

        tokio::select! {
            changed = subscriptions.changed() => {
                if changed.is_err() {
                    break;
                }
                let _ = subscriptions.borrow_and_update();
            }
            changed = rx.changed() => {
                if changed.is_err() {
                    sensor_rx = sensor_snapshot_receiver(&state).await;
                }
            }
            permit = json_tx.reserve() => {
                let Ok(permit) = permit else {
                    break;
                };
                if subscriptions.borrow().contains(TopicId::Sensors) {
                    let snapshot = Arc::clone(&rx.borrow_and_update());
                    if let Some(message) = sensor_snapshot_message(snapshot.as_ref()) {
                        permit.send(message);
                        sent_current_snapshot = true;
                    }
                }
            }
        }
    }
}

pub(super) fn try_enqueue_json<T>(
    json_tx: &tokio::sync::mpsc::Sender<Utf8Bytes>,
    text: T,
    stream: &str,
) -> bool
where
    T: Into<Utf8Bytes>,
{
    match json_tx.try_send(text.into()) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => {
            if stream != "backpressure" {
                note_slow_consumer_drop(stream);
            }
            false
        }
        Err(TrySendError::Closed(_)) => false,
    }
}

/// Aggregate slow-consumer drops into one periodic summary per stream.
///
/// Dropping is the intended backpressure for live telemetry (newest
/// frame wins), so at 30 fps a slow consumer used to produce a log line
/// per dropped frame, which buries everything else at debug level. The
/// counter keeps every drop; the log line appears at most once per
/// stream per window, carrying the count.
fn note_slow_consumer_drop(stream: &str) {
    const SUMMARY_WINDOW: Duration = Duration::from_secs(10);
    static DROPS: std::sync::Mutex<Option<HashMap<String, (Instant, u64)>>> =
        std::sync::Mutex::new(None);

    let mut drops = DROPS.lock().expect("slow-consumer drop counter poisoned");
    let drops = drops.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    // Back-dating the first window start makes the very first drop log
    // immediately; after that, summaries wait out a full window.
    let entry = drops
        .entry(stream.to_owned())
        .or_insert_with(|| (now.checked_sub(SUMMARY_WINDOW).unwrap_or(now), 0));
    entry.1 += 1;
    if now.duration_since(entry.0) >= SUMMARY_WINDOW {
        debug!(
            stream,
            dropped = entry.1,
            window_secs = SUMMARY_WINDOW.as_secs(),
            "Dropped queued WebSocket JSON messages for slow consumer"
        );
        *entry = (now, 0);
    }
}

fn should_emit(last_sent: &mut Instant, fps: u32) -> bool {
    let clamped_fps = fps.max(1);
    let interval = Duration::from_secs_f64(1.0 / f64::from(clamped_fps));
    let now = Instant::now();
    if now.duration_since(*last_sent) < interval {
        return false;
    }
    *last_sent = now;
    true
}

fn preview_initial_last_sent() -> Instant {
    Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now)
}

fn preview_send_delay(last_sent: Instant, fps: u32, now: Instant) -> Duration {
    let clamped_fps = fps.max(1);
    let interval = Duration::from_secs_f64(1.0 / f64::from(clamped_fps));
    interval.saturating_sub(now.saturating_duration_since(last_sent))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PreviewSurfaceIdentity {
    generation: u64,
    storage: PublishedSurfaceStorageIdentity,
    width: u32,
    height: u32,
}

fn preview_surface_identity(frame: &hypercolor_core::bus::CanvasFrame) -> PreviewSurfaceIdentity {
    PreviewSurfaceIdentity {
        generation: frame.surface().generation(),
        storage: frame.surface().storage_identity(),
        width: frame.width,
        height: frame.height,
    }
}

async fn enqueue_backpressure_notice(
    json_tx: &tokio::sync::mpsc::Sender<Utf8Bytes>,
    topic: &str,
    key: Option<&str>,
    advice: BackpressureAdvice,
    dropped_frames: u32,
) -> bool {
    let suggested_fps = match advice {
        BackpressureAdvice::ReduceFps(current_fps) => {
            f64::from(current_fps.saturating_div(2).max(1))
        }
        BackpressureAdvice::ReduceCadence(cadence) => cadence.fps(),
    };
    let message = ServerMessage::Backpressure {
        dropped_frames: dropped_frames.max(1),
        topic: topic.to_owned(),
        key: key.map(str::to_owned),
        recommendation: "reduce_fps".to_owned(),
        suggested_fps: Some(suggested_fps),
    };

    let Ok(text) = serde_json::to_string(&message) else {
        return false;
    };
    json_tx.send(text.into()).await.is_ok()
}

#[expect(
    clippy::too_many_lines,
    reason = "metrics assembly mirrors the exported payload shape for the WebSocket protocol"
)]
pub(super) async fn build_metrics_message(
    state: &AppState,
    bytes_sent_per_sec: f64,
) -> ServerMessage {
    let (render_stats, render_elapsed_ms) = {
        let render_loop = state.render_loop.read().await;
        (
            render_loop.stats(),
            render_loop.elapsed().as_secs_f64() * 1000.0,
        )
    };
    let performance_snapshot = state.performance.read().await.snapshot();
    let render_active = render_stats.state == RenderLoopState::Running;
    let target_fps = render_stats.tier.fps();
    let ceiling_fps = render_stats.max_tier.fps();
    let avg_frame_secs = render_stats.avg_frame_time.as_secs_f64();
    let capacity_fps = if render_active {
        paced_fps(avg_frame_secs, target_fps)
    } else {
        0.0
    };
    let avg_ms = if render_active {
        avg_frame_secs * 1000.0
    } else {
        0.0
    };
    let frame_time = frame_time_summary(
        if render_active {
            performance_snapshot.frame_time
        } else {
            RenderFrameTimeSummary::default()
        },
        avg_ms,
    );
    let latest_frame = if render_active {
        performance_snapshot.latest_frame.unwrap_or_default()
    } else {
        LatestFrameMetrics::default()
    };
    let frame_age_ms = if latest_frame.timestamp_ms > 0 {
        (render_elapsed_ms - f64::from(latest_frame.timestamp_ms)).max(0.0)
    } else {
        0.0
    };

    let devices = state.device_registry.list().await;
    let total_leds = devices.iter().fold(0_usize, |acc, tracked| {
        let led_count = usize::try_from(tracked.info.total_led_count()).unwrap_or_default();
        acc.saturating_add(led_count)
    });
    let connected = devices.len();

    let (canvas_width, canvas_height) = {
        let spatial = state.spatial_engine.read().await;
        let layout = spatial.layout();
        (layout.canvas_width, layout.canvas_height)
    };
    let canvas_buffer_bytes = u64::from(canvas_width)
        .saturating_mul(u64::from(canvas_height))
        .saturating_mul(WS_CANVAS_BYTES_PER_PIXEL_RGBA);
    let canvas_buffer_kb = u32::try_from(canvas_buffer_bytes / 1024).unwrap_or(u32::MAX);

    let daemon_rss_mb = process_rss_mb().unwrap_or(0.0);
    let client_count = WS_CLIENT_COUNT.load(Ordering::Relaxed);
    let preview_runtime = state.preview_runtime.snapshot();
    let canvas_demand = state.preview_runtime.canvas_demand();
    let scene_canvas_demand = state.preview_runtime.scene_canvas_demand();
    let screen_canvas_demand = state.preview_runtime.screen_canvas_demand();
    let web_viewport_canvas_demand = state.preview_runtime.web_viewport_canvas_demand();
    let zone_preview_demand = state.preview_runtime.zone_preview_demand();
    let display_output = state.display_frames.read().await.metrics_snapshot();
    let servo_health = servo_effect_health_counts();
    let pipeline_health = render_pipeline_health_counts();
    let usb_actor_metrics = usb_actor_metrics_snapshot();

    ServerMessage::Metrics {
        timestamp: format_iso8601_now(),
        data: MetricsPayload {
            fps: MetricsFps {
                target: target_fps,
                ceiling: ceiling_fps,
                capacity: round_1(capacity_fps),
                delivered: if render_active {
                    round_1(performance_snapshot.delivered_fps)
                } else {
                    0.0
                },
                dropped: render_stats.consecutive_misses,
            },
            frame_time: MetricsFrameTime {
                avg_ms: round_2(frame_time.avg_ms),
                p95_ms: round_2(frame_time.p95_ms),
                p99_ms: round_2(frame_time.p99_ms),
                max_ms: round_2(frame_time.max_ms),
            },
            input_latency: MetricsSessionLatency {
                sample_count: performance_snapshot.input_time_sample_count,
                avg_ms: round_2(performance_snapshot.input_time.avg_ms),
                p95_ms: round_2(performance_snapshot.input_time.p95_ms),
                p99_ms: round_2(performance_snapshot.input_time.p99_ms),
                max_ms: round_2(performance_snapshot.input_time.max_ms),
            },
            stages: MetricsStages {
                input_sampling_ms: round_2(us_to_ms(latest_frame.input_us)),
                producer_rendering_ms: round_2(us_to_ms(latest_frame.producer_us)),
                producer_effect_rendering_ms: round_2(us_to_ms(latest_frame.producer_render_us)),
                producer_scene_compose_ms: round_2(us_to_ms(
                    latest_frame.producer_scene_compose_us,
                )),
                composition_ms: round_2(us_to_ms(latest_frame.composition_us)),
                effect_rendering_ms: round_2(us_to_ms(latest_frame.render_us)),
                spatial_sampling_ms: round_2(us_to_ms(latest_frame.sample_us)),
                device_output_ms: round_2(us_to_ms(latest_frame.push_us)),
                preview_postprocess_ms: round_2(us_to_ms(latest_frame.postprocess_us)),
                event_bus_ms: round_2(us_to_ms(latest_frame.publish_us)),
                publish_frame_data_ms: round_2(us_to_ms(latest_frame.publish_frame_data_us)),
                publish_group_canvas_ms: round_2(us_to_ms(latest_frame.publish_group_canvas_us)),
                publish_preview_ms: round_2(us_to_ms(latest_frame.publish_preview_us)),
                publish_events_ms: round_2(us_to_ms(latest_frame.publish_events_us)),
                coordination_overhead_ms: round_2(us_to_ms(latest_frame.overhead_us)),
            },
            pacing: MetricsPacing {
                jitter_avg_ms: round_2(performance_snapshot.pacing.jitter_avg_ms),
                jitter_p95_ms: round_2(performance_snapshot.pacing.jitter_p95_ms),
                jitter_max_ms: round_2(performance_snapshot.pacing.jitter_max_ms),
                wake_delay_avg_ms: round_2(performance_snapshot.pacing.wake_delay_avg_ms),
                wake_delay_p95_ms: round_2(performance_snapshot.pacing.wake_delay_p95_ms),
                wake_delay_max_ms: round_2(performance_snapshot.pacing.wake_delay_max_ms),
                push_avg_ms: round_2(performance_snapshot.pacing.push_avg_ms),
                push_p95_ms: round_2(performance_snapshot.pacing.push_p95_ms),
                push_max_ms: round_2(performance_snapshot.pacing.push_max_ms),
                publish_avg_ms: round_2(performance_snapshot.pacing.publish_avg_ms),
                publish_p95_ms: round_2(performance_snapshot.pacing.publish_p95_ms),
                publish_max_ms: round_2(performance_snapshot.pacing.publish_max_ms),
                frame_age_ms: round_2(frame_age_ms),
                reused_inputs: performance_snapshot.pacing.reused_inputs,
                reused_canvas: performance_snapshot.pacing.reused_canvas,
                retained_effect: performance_snapshot.pacing.retained_effect,
                retained_screen: performance_snapshot.pacing.retained_screen,
                composition_bypassed: performance_snapshot.pacing.composition_bypassed,
                gpu_zone_sampling: performance_snapshot.pacing.gpu_zone_sampling,
                gpu_sample_deferred: performance_snapshot.pacing.gpu_sample_deferred,
                gpu_sample_stale: performance_snapshot.pacing.gpu_sample_stale,
                gpu_sample_retry_hit: performance_snapshot.pacing.gpu_sample_retry_hit,
                gpu_sample_queue_saturated: performance_snapshot.pacing.gpu_sample_queue_saturated,
                gpu_sample_wait_blocked: performance_snapshot.pacing.gpu_sample_wait_blocked,
                gpu_sample_cpu_fallback: performance_snapshot.pacing.gpu_sample_cpu_fallback,
                preview_surface: performance_snapshot.pacing.preview_surface,
                scene_canvas_forced_surface: performance_snapshot
                    .pacing
                    .scene_canvas_forced_surface,
                gpu_readback_failed_frames: performance_snapshot.pacing.gpu_readback_failed_frames,
                output_error_frames: performance_snapshot.pacing.output_error_frames,
                full_frame_copy_frames: performance_snapshot.pacing.full_frame_copy_frames,
                output_current_frame: performance_snapshot.pacing.output_current_frame,
                output_published_frame: performance_snapshot.pacing.output_published_frame,
                output_routed_reuse: performance_snapshot.pacing.output_routed_reuse,
                output_reused_published_frame: performance_snapshot
                    .pacing
                    .output_reused_published_frame,
            },
            effect_health: MetricsEffectHealth {
                errors_total: performance_snapshot.effect_health.errors_total,
                fallbacks_applied_total: performance_snapshot.effect_health.fallbacks_applied_total,
                producer_gpu_readback_failures_total: performance_snapshot
                    .effect_health
                    .producer_gpu_readback_failures_total,
                servo_soft_stalls_total: servo_health.soft_stalls_total,
                servo_breaker_opens_total: servo_health.breaker_opens_total,
                servo_session_creates_total: servo_health.session_creates_total,
                servo_session_create_failures_total: servo_health.session_create_failures_total,
                servo_session_create_wait_total_ms: us_to_ms_f64(
                    servo_health.session_create_wait_total_us,
                ),
                servo_session_create_wait_max_ms: us_to_ms_f64(
                    servo_health.session_create_wait_max_us,
                ),
                servo_page_loads_total: servo_health.page_loads_total,
                servo_page_load_failures_total: servo_health.page_load_failures_total,
                servo_page_load_wait_total_ms: us_to_ms_f64(servo_health.page_load_wait_total_us),
                servo_page_load_wait_max_ms: us_to_ms_f64(servo_health.page_load_wait_max_us),
                servo_renderer_loads_total: servo_health.renderer_loads_total,
                servo_renderer_load_failures_total: servo_health.renderer_load_failures_total,
                servo_renderer_load_wait_total_ms: us_to_ms_f64(
                    servo_health.renderer_load_wait_total_us,
                ),
                servo_renderer_load_wait_max_ms: us_to_ms_f64(
                    servo_health.renderer_load_wait_max_us,
                ),
                servo_detached_destroys_total: servo_health.detached_destroys_total,
                servo_detached_destroy_failures_total: servo_health.detached_destroy_failures_total,
                servo_destroy_wait_total_ms: us_to_ms_f64(servo_health.destroy_wait_total_us),
                servo_destroy_wait_max_ms: us_to_ms_f64(servo_health.destroy_wait_max_us),
                servo_render_requests_total: servo_health.render_requests_total,
                servo_render_queue_wait_total_ms: us_to_ms_f64(
                    servo_health.render_queue_wait_total_us,
                ),
                servo_render_queue_wait_max_ms: us_to_ms_f64(servo_health.render_queue_wait_max_us),
                servo_render_scene_requests_total: servo_health.render_scene_requests_total,
                servo_render_scene_queue_wait_total_ms: us_to_ms_f64(
                    servo_health.render_scene_queue_wait_total_us,
                ),
                servo_render_scene_queue_wait_max_ms: us_to_ms_f64(
                    servo_health.render_scene_queue_wait_max_us,
                ),
                servo_render_display_requests_total: servo_health.render_display_requests_total,
                servo_render_display_queue_wait_total_ms: us_to_ms_f64(
                    servo_health.render_display_queue_wait_total_us,
                ),
                servo_render_display_queue_wait_max_ms: us_to_ms_f64(
                    servo_health.render_display_queue_wait_max_us,
                ),
                servo_render_queue_depth: servo_health.render_queue_depth,
                servo_render_queue_depth_max: servo_health.render_queue_depth_max,
                servo_render_superseded_total: servo_health.render_superseded_total,
                servo_render_pending_age_max_ms: us_to_ms_f64(
                    servo_health.render_pending_age_max_us,
                ),
                servo_render_cpu_frames_total: servo_health.render_cpu_frames_total,
                servo_render_cached_frames_total: servo_health.render_cached_frames_total,
                servo_render_gpu_frames_total: servo_health.render_gpu_frames_total,
                servo_gpu_import_failures_total: servo_health.render_gpu_import_failures_total,
                servo_gpu_import_fallbacks_total: servo_health.render_gpu_import_fallbacks_total,
                servo_gpu_import_fallback_reason: servo_health.render_gpu_import_fallback_reason,
                servo_gpu_import_windows_sync_mode: servo_health
                    .render_gpu_import_windows_sync_mode,
                servo_gpu_import_stale_frame_total: servo_health
                    .render_gpu_import_stale_frame_total,
                servo_gpu_import_adapter_mismatch_total: servo_health
                    .render_gpu_import_adapter_mismatch_total,
                servo_gpu_import_slot_count: servo_health.render_gpu_import_slot_count,
                servo_gpu_import_pending_slots: servo_health.render_gpu_import_pending_slots,
                servo_gpu_import_pending_slots_max: servo_health
                    .render_gpu_import_pending_slots_max,
                servo_gpu_import_completed_slots: servo_health.render_gpu_import_completed_slots,
                servo_gpu_import_available_slots: servo_health.render_gpu_import_available_slots,
                servo_gpu_import_available_slots_min: servo_health
                    .render_gpu_import_available_slots_min,
                servo_gpu_import_oldest_pending_age_max_ms: us_to_ms_f64(
                    servo_health.render_gpu_import_oldest_pending_age_max_us,
                ),
                servo_gpu_import_blit_total_ms: us_to_ms_f64(
                    servo_health.render_gpu_import_blit_total_us,
                ),
                servo_gpu_import_blit_max_ms: us_to_ms_f64(
                    servo_health.render_gpu_import_blit_max_us,
                ),
                servo_gpu_import_sync_total_ms: us_to_ms_f64(
                    servo_health.render_gpu_import_sync_total_us,
                ),
                servo_gpu_import_sync_max_ms: us_to_ms_f64(
                    servo_health.render_gpu_import_sync_max_us,
                ),
                servo_gpu_import_total_ms: us_to_ms_f64(servo_health.render_gpu_import_total_us),
                servo_gpu_import_max_ms: us_to_ms_f64(servo_health.render_gpu_import_max_us),
                producer_cpu_frames_total: pipeline_health.cpu_producer_frames,
                producer_gpu_frames_total: pipeline_health.gpu_producer_frames,
                producer_gpu_cpu_materialization_blocked_total: pipeline_health
                    .gpu_cpu_materialization_blocked_total,
                sparkleflinger_gpu_source_upload_skipped_total: pipeline_health
                    .skipped_gpu_source_uploads,
                sparkleflinger_media_texture_allocations_total: pipeline_health
                    .media_texture_allocations_total,
                sparkleflinger_media_texture_upload_bytes_total: pipeline_health
                    .media_texture_upload_bytes_total,
                sparkleflinger_display_finalize_rgba_attempts_total: pipeline_health
                    .display_finalize_rgba_attempts_total,
                sparkleflinger_display_finalize_yuv_attempts_total: pipeline_health
                    .display_finalize_yuv_attempts_total,
                sparkleflinger_display_finalize_successes_total: pipeline_health
                    .display_finalize_successes_total,
                sparkleflinger_display_finalize_misses_total: pipeline_health
                    .display_finalize_misses_total,
                sparkleflinger_display_finalize_latches_total: pipeline_health
                    .display_finalize_latches_total,
                sparkleflinger_display_finalize_blocking_wait_total_ms: us_to_ms_f64(
                    pipeline_health.display_finalize_blocking_wait_total_us,
                ),
                sparkleflinger_display_finalize_blocking_wait_max_ms: us_to_ms_f64(
                    pipeline_health.display_finalize_blocking_wait_max_us,
                ),
                sparkleflinger_display_finalize_surface_reallocs_total: pipeline_health
                    .display_finalize_surface_reallocs_total,
                servo_render_evaluate_scripts_total_ms: us_to_ms_f64(
                    servo_health.render_evaluate_scripts_total_us,
                ),
                servo_render_evaluate_scripts_max_ms: us_to_ms_f64(
                    servo_health.render_evaluate_scripts_max_us,
                ),
                servo_render_event_loop_total_ms: us_to_ms_f64(
                    servo_health.render_event_loop_total_us,
                ),
                servo_render_event_loop_max_ms: us_to_ms_f64(servo_health.render_event_loop_max_us),
                servo_render_paint_total_ms: us_to_ms_f64(servo_health.render_paint_total_us),
                servo_render_paint_max_ms: us_to_ms_f64(servo_health.render_paint_max_us),
                servo_render_readback_total_ms: us_to_ms_f64(servo_health.render_readback_total_us),
                servo_render_readback_max_ms: us_to_ms_f64(servo_health.render_readback_max_us),
                servo_render_frame_total_ms: us_to_ms_f64(servo_health.render_frame_total_us),
                servo_render_frame_max_ms: us_to_ms_f64(servo_health.render_frame_max_us),
            },
            timeline: MetricsTimeline {
                frame_token: latest_frame.timeline.frame_token,
                compositor_backend: latest_frame.compositor_backend.as_str().to_owned(),
                output_frame_source: latest_frame.output_frame_source.as_str().to_owned(),
                output_reuses_published_frame: latest_frame.output_reuses_published_frame,
                output_brightness_bits: latest_frame.output_brightness_bits,
                output_brightness_generation: latest_frame.output_brightness_generation,
                output_routing_signature: latest_frame.output_routing_signature,
                output_zone_shape_signature: latest_frame.output_zone_shape_signature,
                output_unassigned_behavior_generation: latest_frame
                    .output_unassigned_behavior_generation,
                devices_written: latest_frame.devices_written,
                total_leds: latest_frame.total_leds,
                gpu_zone_sampling: latest_frame.gpu_zone_sampling,
                gpu_sample_deferred: latest_frame.gpu_sample_deferred,
                gpu_sample_stale: latest_frame.gpu_sample_stale,
                gpu_sample_retry_hit: latest_frame.gpu_sample_retry_hit,
                gpu_sample_queue_saturated: latest_frame.gpu_sample_queue_saturated,
                gpu_sample_wait_blocked: latest_frame.gpu_sample_wait_blocked,
                gpu_sample_cpu_fallback: latest_frame.gpu_sample_cpu_fallback,
                preview_surface: latest_frame.preview_surface,
                scene_canvas_forced_surface: latest_frame.scene_canvas_forced_surface,
                cpu_readback_skipped: latest_frame.cpu_readback_skipped,
                gpu_readback_failed: latest_frame.gpu_readback_failed,
                budget_ms: round_2(us_to_ms(latest_frame.timeline.budget_us)),
                wake_late_ms: round_2(us_to_ms(latest_frame.wake_late_us)),
                logical_layer_count: latest_frame.logical_layer_count,
                render_group_count: latest_frame.render_group_count,
                scene_active: latest_frame.scene_active,
                scene_transition_active: latest_frame.scene_transition_active,
                scene_snapshot_done_ms: round_2(us_to_ms(
                    latest_frame.timeline.scene_snapshot_done_us,
                )),
                input_done_ms: round_2(us_to_ms(latest_frame.timeline.input_done_us)),
                deferred_sample_ms: round_2(us_to_ms(latest_frame.deferred_sample_us)),
                producer_done_ms: round_2(us_to_ms(latest_frame.timeline.producer_done_us)),
                composition_done_ms: round_2(us_to_ms(latest_frame.timeline.composition_done_us)),
                preview_advance_ms: round_2(us_to_ms(latest_frame.preview_advance_us)),
                sampling_done_ms: round_2(us_to_ms(latest_frame.timeline.sample_done_us)),
                output_done_ms: round_2(us_to_ms(latest_frame.timeline.output_done_us)),
                publish_done_ms: round_2(us_to_ms(latest_frame.timeline.publish_done_us)),
                frame_done_ms: round_2(us_to_ms(latest_frame.timeline.frame_done_us)),
            },
            render_surfaces: MetricsRenderSurfaces {
                canvas_receivers: latest_frame.canvas_receiver_count,
                scene_pool_saturation_reallocs: latest_frame.scene_pool_saturation_reallocs,
                direct_pool_saturation_reallocs: latest_frame.direct_pool_saturation_reallocs,
                scene_pool_grown_slots: latest_frame.scene_pool_grown_slots,
                direct_pool_grown_slots: latest_frame.direct_pool_grown_slots,
                scene_pool_slot_count: latest_frame.scene_pool_slot_count,
                scene_pool_max_slots: latest_frame.scene_pool_max_slots,
                direct_pool_slot_count: latest_frame.direct_pool_slot_count,
                direct_pool_max_slots: latest_frame.direct_pool_max_slots,
                scene_pool_shared_published_slots: latest_frame.scene_pool_shared_published_slots,
                scene_pool_max_ref_count: latest_frame.scene_pool_max_ref_count,
                direct_pool_shared_published_slots: latest_frame.direct_pool_shared_published_slots,
                direct_pool_max_ref_count: latest_frame.direct_pool_max_ref_count,
                scene_pool_free_slots: latest_frame.scene_pool_free_slots,
                scene_pool_published_slots: latest_frame.scene_pool_published_slots,
                scene_pool_dequeued_slots: latest_frame.scene_pool_dequeued_slots,
                direct_pool_free_slots: latest_frame.direct_pool_free_slots,
                direct_pool_published_slots: latest_frame.direct_pool_published_slots,
                direct_pool_dequeued_slots: latest_frame.direct_pool_dequeued_slots,
                preview_pool_slot_count: latest_frame.preview_pool_slot_count,
                preview_pool_free_slots: latest_frame.preview_pool_free_slots,
                preview_pool_published_slots: latest_frame.preview_pool_published_slots,
                preview_pool_dequeued_slots: latest_frame.preview_pool_dequeued_slots,
                compositor_pool_slot_count: latest_frame.compositor_pool_slot_count,
                compositor_pool_free_slots: latest_frame.compositor_pool_free_slots,
                compositor_pool_published_slots: latest_frame.compositor_pool_published_slots,
                compositor_pool_dequeued_slots: latest_frame.compositor_pool_dequeued_slots,
            },
            preview: MetricsPreview {
                canvas_receivers: preview_runtime.canvas_receivers,
                scene_canvas_receivers: preview_runtime.scene_canvas_receivers,
                screen_canvas_receivers: preview_runtime.screen_canvas_receivers,
                web_viewport_canvas_receivers: preview_runtime.web_viewport_canvas_receivers,
                zone_preview_receivers: preview_runtime.zone_preview_receivers,
                canvas_frames_published: preview_runtime.canvas_frames_published,
                scene_canvas_frames_published: preview_runtime.scene_canvas_frames_published,
                screen_canvas_frames_published: preview_runtime.screen_canvas_frames_published,
                web_viewport_canvas_frames_published: preview_runtime
                    .web_viewport_canvas_frames_published,
                zone_preview_frames_published: preview_runtime.zone_preview_frames_published,
                latest_canvas_frame_number: preview_runtime.latest_canvas_frame_number,
                latest_scene_canvas_frame_number: preview_runtime.latest_scene_canvas_frame_number,
                latest_screen_canvas_frame_number: preview_runtime
                    .latest_screen_canvas_frame_number,
                latest_web_viewport_canvas_frame_number: preview_runtime
                    .latest_web_viewport_canvas_frame_number,
                latest_zone_preview_frame_number: preview_runtime.latest_zone_preview_frame_number,
                canvas_demand: metrics_preview_demand(canvas_demand),
                scene_canvas_demand: metrics_preview_demand(scene_canvas_demand),
                screen_canvas_demand: metrics_preview_demand(screen_canvas_demand),
                web_viewport_canvas_demand: metrics_preview_demand(web_viewport_canvas_demand),
                zone_preview_demand: metrics_preview_demand(zone_preview_demand),
            },
            display_output: MetricsDisplayOutput {
                captured_devices: display_output.captured_devices,
                preview_subscribers: display_output.preview_subscribers,
                write_attempts_total: display_output.write_attempts_total,
                write_successes_total: display_output.write_successes_total,
                write_failures_total: display_output.write_failures_total,
                retry_attempts_total: display_output.retry_attempts_total,
                display_lane: MetricsDisplayLane {
                    display_frames_total: usb_actor_metrics.display_frames_total,
                    display_frames_delayed_for_led_total: usb_actor_metrics
                        .display_frames_delayed_for_led_total,
                    display_led_priority_wait_total_ms: us_to_ms_f64(
                        usb_actor_metrics.display_led_priority_wait_total_us,
                    ),
                    display_led_priority_wait_max_ms: us_to_ms_f64(
                        usb_actor_metrics.display_led_priority_wait_max_us,
                    ),
                },
                last_failure_age_ms: display_output.last_failure_age_ms,
            },
            copies: MetricsCopies {
                full_frame_count: latest_frame.full_frame_copy_count,
                full_frame_kb: round_2(bytes_to_kib(latest_frame.full_frame_copy_bytes)),
                producer_full_frame_count: latest_frame.producer_full_frame_copy.count,
                producer_full_frame_kb: round_2(bytes_to_kib(
                    latest_frame.producer_full_frame_copy.bytes,
                )),
                producer_reason: latest_frame.producer_full_frame_copy.reason,
                publication_full_frame_count: latest_frame.publication_full_frame_copy.count,
                publication_full_frame_kb: round_2(bytes_to_kib(
                    latest_frame.publication_full_frame_copy.bytes,
                )),
                publication_reason: latest_frame.publication_full_frame_copy.reason,
                session_full_frame_count: performance_snapshot.full_frame_copy_count_total,
                session_full_frame_frames: performance_snapshot.full_frame_copy_frames_total,
                session_full_frame_bytes: performance_snapshot.full_frame_copy_bytes_total,
            },
            memory: MetricsMemory {
                daemon_rss_mb: round_1(daemon_rss_mb),
                servo_rss_mb: 0.0,
                canvas_buffer_kb,
            },
            devices: MetricsDevices {
                connected,
                total_leds,
                output_errors: latest_frame.output_errors,
            },
            websocket: MetricsWebsocket {
                client_count,
                bytes_sent_per_sec: round_1(bytes_sent_per_sec),
                frame_payload_builds: WS_FRAME_PAYLOAD_BUILD_COUNT.load(Ordering::Relaxed),
                frame_payload_cache_hits: WS_FRAME_PAYLOAD_CACHE_HIT_COUNT.load(Ordering::Relaxed),
                canvas_payload_builds: WS_CANVAS_PAYLOAD_BUILD_COUNT.load(Ordering::Relaxed),
                canvas_payload_cache_hits: WS_CANVAS_PAYLOAD_CACHE_HIT_COUNT
                    .load(Ordering::Relaxed),
                preview_publications_queued: WS_PREVIEW_PUBLICATION_QUEUED_COUNT
                    .load(Ordering::Relaxed),
                preview_publications_replaced: WS_PREVIEW_PUBLICATION_REPLACED_COUNT
                    .load(Ordering::Relaxed),
                preview_publications_evicted: WS_PREVIEW_PUBLICATION_EVICTED_COUNT
                    .load(Ordering::Relaxed),
                preview_publications_rejected: WS_PREVIEW_PUBLICATION_REJECTED_COUNT
                    .load(Ordering::Relaxed),
                preview_publications_sent: WS_PREVIEW_PUBLICATION_SENT_COUNT
                    .load(Ordering::Relaxed),
                preview_chunks_sent: WS_PREVIEW_CHUNK_SENT_COUNT.load(Ordering::Relaxed),
                preview_queue_bytes: WS_PREVIEW_QUEUE_BYTES.load(Ordering::Relaxed),
            },
        },
    }
}

pub(super) fn build_device_metrics_message(state: &AppState) -> ServerMessage {
    let snapshot = state.device_metrics.load_full();
    ServerMessage::DeviceMetrics {
        timestamp: format_iso8601_now(),
        data: snapshot.as_ref().clone(),
    }
}

async fn sensor_snapshot_receiver(
    state: &AppState,
) -> Option<watch::Receiver<Arc<SystemSnapshot>>> {
    let input_manager = state.input_manager.lock().await;
    input_manager.sensor_snapshot_receiver()
}

fn sensor_snapshot_message(snapshot: &SystemSnapshot) -> Option<Utf8Bytes> {
    let message = ServerMessage::Sensors {
        timestamp: format_iso8601_now(),
        data: snapshot.clone(),
    };
    serde_json::to_string(&message).ok().map(Into::into)
}

#[derive(Debug, Clone, Copy, Default)]
struct ServoEffectHealthCounts {
    soft_stalls_total: u64,
    breaker_opens_total: u64,
    session_creates_total: u64,
    session_create_failures_total: u64,
    session_create_wait_total_us: u64,
    session_create_wait_max_us: u64,
    page_loads_total: u64,
    page_load_failures_total: u64,
    page_load_wait_total_us: u64,
    page_load_wait_max_us: u64,
    renderer_loads_total: u64,
    renderer_load_failures_total: u64,
    renderer_load_wait_total_us: u64,
    renderer_load_wait_max_us: u64,
    detached_destroys_total: u64,
    detached_destroy_failures_total: u64,
    destroy_wait_total_us: u64,
    destroy_wait_max_us: u64,
    render_requests_total: u64,
    render_queue_wait_total_us: u64,
    render_queue_wait_max_us: u64,
    render_scene_requests_total: u64,
    render_scene_queue_wait_total_us: u64,
    render_scene_queue_wait_max_us: u64,
    render_display_requests_total: u64,
    render_display_queue_wait_total_us: u64,
    render_display_queue_wait_max_us: u64,
    render_queue_depth: u64,
    render_queue_depth_max: u64,
    render_superseded_total: u64,
    render_pending_age_max_us: u64,
    render_cpu_frames_total: u64,
    render_cached_frames_total: u64,
    render_gpu_frames_total: u64,
    render_gpu_import_failures_total: u64,
    render_gpu_import_fallbacks_total: u64,
    render_gpu_import_fallback_reason: Option<&'static str>,
    render_gpu_import_windows_sync_mode: Option<&'static str>,
    render_gpu_import_stale_frame_total: u64,
    render_gpu_import_adapter_mismatch_total: u64,
    render_gpu_import_slot_count: u64,
    render_gpu_import_pending_slots: u64,
    render_gpu_import_pending_slots_max: u64,
    render_gpu_import_completed_slots: u64,
    render_gpu_import_available_slots: u64,
    render_gpu_import_available_slots_min: u64,
    render_gpu_import_oldest_pending_age_max_us: u64,
    render_gpu_import_blit_total_us: u64,
    render_gpu_import_blit_max_us: u64,
    render_gpu_import_sync_total_us: u64,
    render_gpu_import_sync_max_us: u64,
    render_gpu_import_total_us: u64,
    render_gpu_import_max_us: u64,
    render_evaluate_scripts_total_us: u64,
    render_evaluate_scripts_max_us: u64,
    render_event_loop_total_us: u64,
    render_event_loop_max_us: u64,
    render_paint_total_us: u64,
    render_paint_max_us: u64,
    render_readback_total_us: u64,
    render_readback_max_us: u64,
    render_frame_total_us: u64,
    render_frame_max_us: u64,
}

#[cfg(feature = "servo")]
fn servo_effect_health_counts() -> ServoEffectHealthCounts {
    let snapshot = hypercolor_core::effect::servo_telemetry_snapshot();
    ServoEffectHealthCounts {
        soft_stalls_total: snapshot.soft_stalls_total,
        breaker_opens_total: snapshot.breaker_opens_total,
        session_creates_total: snapshot.session_creates_total,
        session_create_failures_total: snapshot.session_create_failures_total,
        session_create_wait_total_us: snapshot.session_create_wait_total_us,
        session_create_wait_max_us: snapshot.session_create_wait_max_us,
        page_loads_total: snapshot.page_loads_total,
        page_load_failures_total: snapshot.page_load_failures_total,
        page_load_wait_total_us: snapshot.page_load_wait_total_us,
        page_load_wait_max_us: snapshot.page_load_wait_max_us,
        renderer_loads_total: snapshot.renderer_loads_total,
        renderer_load_failures_total: snapshot.renderer_load_failures_total,
        renderer_load_wait_total_us: snapshot.renderer_load_wait_total_us,
        renderer_load_wait_max_us: snapshot.renderer_load_wait_max_us,
        detached_destroys_total: snapshot.detached_destroys_total,
        detached_destroy_failures_total: snapshot.detached_destroy_failures_total,
        destroy_wait_total_us: snapshot.destroy_wait_total_us,
        destroy_wait_max_us: snapshot.destroy_wait_max_us,
        render_requests_total: snapshot.render_requests_total,
        render_queue_wait_total_us: snapshot.render_queue_wait_total_us,
        render_queue_wait_max_us: snapshot.render_queue_wait_max_us,
        render_scene_requests_total: snapshot.render_scene_requests_total,
        render_scene_queue_wait_total_us: snapshot.render_scene_queue_wait_total_us,
        render_scene_queue_wait_max_us: snapshot.render_scene_queue_wait_max_us,
        render_display_requests_total: snapshot.render_display_requests_total,
        render_display_queue_wait_total_us: snapshot.render_display_queue_wait_total_us,
        render_display_queue_wait_max_us: snapshot.render_display_queue_wait_max_us,
        render_queue_depth: snapshot.render_queue_depth,
        render_queue_depth_max: snapshot.render_queue_depth_max,
        render_superseded_total: snapshot.render_superseded_total,
        render_pending_age_max_us: snapshot.render_pending_age_max_us,
        render_cpu_frames_total: snapshot.render_cpu_frames_total,
        render_cached_frames_total: snapshot.render_cached_frames_total,
        render_gpu_frames_total: snapshot.render_gpu_frames_total,
        render_gpu_import_failures_total: snapshot.render_gpu_import_failures_total,
        render_gpu_import_fallbacks_total: snapshot.render_gpu_import_fallbacks_total,
        render_gpu_import_fallback_reason: snapshot.render_gpu_import_fallback_reason,
        render_gpu_import_windows_sync_mode: snapshot.render_gpu_import_windows_sync_mode,
        render_gpu_import_stale_frame_total: snapshot.render_gpu_import_stale_frame_total,
        render_gpu_import_adapter_mismatch_total: snapshot.render_gpu_import_adapter_mismatch_total,
        render_gpu_import_slot_count: snapshot.render_gpu_import_slot_count,
        render_gpu_import_pending_slots: snapshot.render_gpu_import_pending_slots,
        render_gpu_import_pending_slots_max: snapshot.render_gpu_import_pending_slots_max,
        render_gpu_import_completed_slots: snapshot.render_gpu_import_completed_slots,
        render_gpu_import_available_slots: snapshot.render_gpu_import_available_slots,
        render_gpu_import_available_slots_min: snapshot.render_gpu_import_available_slots_min,
        render_gpu_import_oldest_pending_age_max_us: snapshot
            .render_gpu_import_oldest_pending_age_max_us,
        render_gpu_import_blit_total_us: snapshot.render_gpu_import_blit_total_us,
        render_gpu_import_blit_max_us: snapshot.render_gpu_import_blit_max_us,
        render_gpu_import_sync_total_us: snapshot.render_gpu_import_sync_total_us,
        render_gpu_import_sync_max_us: snapshot.render_gpu_import_sync_max_us,
        render_gpu_import_total_us: snapshot.render_gpu_import_total_us,
        render_gpu_import_max_us: snapshot.render_gpu_import_max_us,
        render_evaluate_scripts_total_us: snapshot.render_evaluate_scripts_total_us,
        render_evaluate_scripts_max_us: snapshot.render_evaluate_scripts_max_us,
        render_event_loop_total_us: snapshot.render_event_loop_total_us,
        render_event_loop_max_us: snapshot.render_event_loop_max_us,
        render_paint_total_us: snapshot.render_paint_total_us,
        render_paint_max_us: snapshot.render_paint_max_us,
        render_readback_total_us: snapshot.render_readback_total_us,
        render_readback_max_us: snapshot.render_readback_max_us,
        render_frame_total_us: snapshot.render_frame_total_us,
        render_frame_max_us: snapshot.render_frame_max_us,
    }
}

#[cfg(not(feature = "servo"))]
const fn servo_effect_health_counts() -> ServoEffectHealthCounts {
    ServoEffectHealthCounts {
        soft_stalls_total: 0,
        breaker_opens_total: 0,
        session_creates_total: 0,
        session_create_failures_total: 0,
        session_create_wait_total_us: 0,
        session_create_wait_max_us: 0,
        page_loads_total: 0,
        page_load_failures_total: 0,
        page_load_wait_total_us: 0,
        page_load_wait_max_us: 0,
        renderer_loads_total: 0,
        renderer_load_failures_total: 0,
        renderer_load_wait_total_us: 0,
        renderer_load_wait_max_us: 0,
        detached_destroys_total: 0,
        detached_destroy_failures_total: 0,
        destroy_wait_total_us: 0,
        destroy_wait_max_us: 0,
        render_requests_total: 0,
        render_queue_wait_total_us: 0,
        render_queue_wait_max_us: 0,
        render_scene_requests_total: 0,
        render_scene_queue_wait_total_us: 0,
        render_scene_queue_wait_max_us: 0,
        render_display_requests_total: 0,
        render_display_queue_wait_total_us: 0,
        render_display_queue_wait_max_us: 0,
        render_queue_depth: 0,
        render_queue_depth_max: 0,
        render_superseded_total: 0,
        render_pending_age_max_us: 0,
        render_cpu_frames_total: 0,
        render_cached_frames_total: 0,
        render_gpu_frames_total: 0,
        render_gpu_import_failures_total: 0,
        render_gpu_import_fallbacks_total: 0,
        render_gpu_import_fallback_reason: None,
        render_gpu_import_windows_sync_mode: None,
        render_gpu_import_stale_frame_total: 0,
        render_gpu_import_adapter_mismatch_total: 0,
        render_gpu_import_slot_count: 0,
        render_gpu_import_pending_slots: 0,
        render_gpu_import_pending_slots_max: 0,
        render_gpu_import_completed_slots: 0,
        render_gpu_import_available_slots: 0,
        render_gpu_import_available_slots_min: 0,
        render_gpu_import_oldest_pending_age_max_us: 0,
        render_gpu_import_blit_total_us: 0,
        render_gpu_import_blit_max_us: 0,
        render_gpu_import_sync_total_us: 0,
        render_gpu_import_sync_max_us: 0,
        render_gpu_import_total_us: 0,
        render_gpu_import_max_us: 0,
        render_evaluate_scripts_total_us: 0,
        render_evaluate_scripts_max_us: 0,
        render_event_loop_total_us: 0,
        render_event_loop_max_us: 0,
        render_paint_total_us: 0,
        render_paint_max_us: 0,
        render_readback_total_us: 0,
        render_readback_max_us: 0,
        render_frame_total_us: 0,
        render_frame_max_us: 0,
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct RenderPipelineHealthCounts {
    cpu_producer_frames: u64,
    gpu_producer_frames: u64,
    gpu_cpu_materialization_blocked_total: u64,
    skipped_gpu_source_uploads: u64,
    media_texture_allocations_total: u64,
    media_texture_upload_bytes_total: u64,
    display_finalize_rgba_attempts_total: u64,
    display_finalize_yuv_attempts_total: u64,
    display_finalize_successes_total: u64,
    display_finalize_misses_total: u64,
    display_finalize_latches_total: u64,
    display_finalize_blocking_wait_total_us: u64,
    display_finalize_blocking_wait_max_us: u64,
    display_finalize_surface_reallocs_total: u64,
}

fn render_pipeline_health_counts() -> RenderPipelineHealthCounts {
    let producer = crate::render_thread::producer_frame_counts();
    let gpu = gpu_sparkleflinger_health_counts();
    RenderPipelineHealthCounts {
        cpu_producer_frames: producer.cpu_frames,
        gpu_producer_frames: producer.gpu_frames,
        gpu_cpu_materialization_blocked_total: producer.gpu_cpu_materialization_blocked,
        skipped_gpu_source_uploads: gpu.source_upload_skipped_total,
        media_texture_allocations_total: gpu.media_texture_allocations_total,
        media_texture_upload_bytes_total: gpu.media_texture_upload_bytes_total,
        display_finalize_rgba_attempts_total: gpu.display_finalize_rgba_attempts_total,
        display_finalize_yuv_attempts_total: gpu.display_finalize_yuv_attempts_total,
        display_finalize_successes_total: gpu.display_finalize_successes_total,
        display_finalize_misses_total: gpu.display_finalize_misses_total,
        display_finalize_latches_total: gpu.display_finalize_latches_total,
        display_finalize_blocking_wait_total_us: gpu.display_finalize_blocking_wait_total_us,
        display_finalize_blocking_wait_max_us: gpu.display_finalize_blocking_wait_max_us,
        display_finalize_surface_reallocs_total: gpu.display_finalize_surface_reallocs_total,
    }
}

#[cfg(feature = "wgpu")]
fn gpu_sparkleflinger_health_counts() -> GpuSparkleFlingerHealthCounts {
    let snapshot =
        crate::render_thread::sparkleflinger::gpu::gpu_sparkleflinger_telemetry_snapshot();
    GpuSparkleFlingerHealthCounts {
        source_upload_skipped_total: snapshot.source_upload_skipped_total,
        media_texture_allocations_total: snapshot.media_texture_allocations_total,
        media_texture_upload_bytes_total: snapshot.media_texture_upload_bytes_total,
        display_finalize_rgba_attempts_total: snapshot.display_finalize_rgba_attempts_total,
        display_finalize_yuv_attempts_total: snapshot.display_finalize_yuv_attempts_total,
        display_finalize_successes_total: snapshot.display_finalize_successes_total,
        display_finalize_misses_total: snapshot.display_finalize_misses_total,
        display_finalize_latches_total: snapshot.display_finalize_latches_total,
        display_finalize_blocking_wait_total_us: snapshot.display_finalize_blocking_wait_total_us,
        display_finalize_blocking_wait_max_us: snapshot.display_finalize_blocking_wait_max_us,
        display_finalize_surface_reallocs_total: snapshot.display_finalize_surface_reallocs_total,
    }
}

#[cfg(not(feature = "wgpu"))]
const fn gpu_sparkleflinger_health_counts() -> GpuSparkleFlingerHealthCounts {
    GpuSparkleFlingerHealthCounts {
        source_upload_skipped_total: 0,
        media_texture_allocations_total: 0,
        media_texture_upload_bytes_total: 0,
        display_finalize_rgba_attempts_total: 0,
        display_finalize_yuv_attempts_total: 0,
        display_finalize_successes_total: 0,
        display_finalize_misses_total: 0,
        display_finalize_latches_total: 0,
        display_finalize_blocking_wait_total_us: 0,
        display_finalize_blocking_wait_max_us: 0,
        display_finalize_surface_reallocs_total: 0,
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct GpuSparkleFlingerHealthCounts {
    source_upload_skipped_total: u64,
    media_texture_allocations_total: u64,
    media_texture_upload_bytes_total: u64,
    display_finalize_rgba_attempts_total: u64,
    display_finalize_yuv_attempts_total: u64,
    display_finalize_successes_total: u64,
    display_finalize_misses_total: u64,
    display_finalize_latches_total: u64,
    display_finalize_blocking_wait_total_us: u64,
    display_finalize_blocking_wait_max_us: u64,
    display_finalize_surface_reallocs_total: u64,
}

fn round_1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn round_2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn paced_fps(avg_frame_secs: f64, target_fps: u32) -> f64 {
    if avg_frame_secs <= 0.0 {
        return f64::from(target_fps);
    }

    (1.0 / avg_frame_secs).clamp(0.0, f64::from(target_fps))
}

fn us_to_ms(value: u32) -> f64 {
    f64::from(value) / 1000.0
}

fn us_to_ms_f64(value: u64) -> f64 {
    std::time::Duration::from_micros(value).as_secs_f64() * 1000.0
}

fn bytes_to_kib(value: u32) -> f64 {
    f64::from(value) / 1024.0
}

fn frame_time_summary(
    summary: RenderFrameTimeSummary,
    fallback_avg_ms: f64,
) -> RenderFrameTimeSummary {
    if summary.avg_ms > 0.0 {
        summary
    } else {
        RenderFrameTimeSummary {
            avg_ms: fallback_avg_ms,
            p95_ms: fallback_avg_ms,
            p99_ms: fallback_avg_ms,
            max_ms: fallback_avg_ms,
        }
    }
}

fn process_rss_mb() -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
        let kb = line.split_whitespace().nth(1)?.parse::<f64>().ok()?;
        Some(kb / 1024.0)
    }

    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn format_iso8601_now() -> String {
    let now = SystemTime::now();
    let duration = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();

    let total_secs = duration.as_secs();
    let millis = duration.subsec_millis();
    let (year, month, day, hour, minute, second) = epoch_to_utc(total_secs);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

#[expect(clippy::cast_possible_truncation, clippy::as_conversions)]
fn epoch_to_utc(epoch_secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let secs_per_day: u64 = 86400;
    let days = epoch_secs / secs_per_day;
    let day_secs = epoch_secs % secs_per_day;

    let hour = (day_secs / 3600) as u32;
    let minute = ((day_secs % 3600) / 60) as u32;
    let second = (day_secs % 60) as u32;

    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    (y as u32, m as u32, d as u32, hour, minute, second)
}

pub(super) fn publish_subscriptions(
    subscriptions_tx: &watch::Sender<SubscriptionState>,
    subscriptions: &SubscriptionState,
) {
    let _ = subscriptions_tx.send(subscriptions.clone());
}

#[cfg(test)]
mod transport_tests;

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use axum::extract::ws::Utf8Bytes;
    use hypercolor_core::bus::CanvasFrame;
    use hypercolor_types::canvas::{Canvas, PublishedSurface};
    use tokio::sync::mpsc;

    use super::{
        BACKPRESSURE_REPORT_INTERVAL, BackpressureAdvice, BackpressureReporter, Cadence,
        preview_send_delay, preview_surface_identity,
    };

    #[test]
    fn preview_send_delay_is_zero_after_interval_elapses() {
        let now = Instant::now();
        let last_sent = now.checked_sub(Duration::from_millis(100)).unwrap_or(now);

        assert_eq!(preview_send_delay(last_sent, 60, now), Duration::ZERO);
    }

    #[test]
    fn preview_send_delay_returns_remaining_budget() {
        let now = Instant::now();
        let last_sent = now.checked_sub(Duration::from_millis(5)).unwrap_or(now);
        let delay = preview_send_delay(last_sent, 60, now);

        assert!(delay > Duration::ZERO);
        assert!(delay <= Duration::from_millis(12));
    }

    #[test]
    fn preview_surface_identity_ignores_frame_metadata_updates() {
        let surface = PublishedSurface::from_owned_canvas(Canvas::new(2, 1), 7, 99);
        let first = CanvasFrame::from_surface(surface.clone());
        let second = CanvasFrame::from_surface(surface.with_frame_metadata(8, 100));

        assert_eq!(
            preview_surface_identity(&first),
            preview_surface_identity(&second)
        );
    }

    #[test]
    fn preview_surface_identity_keeps_empty_frames_stable() {
        assert_eq!(
            preview_surface_identity(&CanvasFrame::empty()),
            preview_surface_identity(&CanvasFrame::empty())
        );
    }

    #[tokio::test]
    async fn backpressure_reporter_batches_drops_inside_interval() {
        let (json_tx, mut json_rx) = mpsc::channel::<Utf8Bytes>(8);
        let reporter = BackpressureReporter::new(json_tx, "canvas", None);

        reporter.record_drop(BackpressureAdvice::ReduceFps(60));
        let first = json_rx
            .recv()
            .await
            .expect("first notice should send immediately");
        let first: serde_json::Value =
            serde_json::from_str(first.as_str()).expect("first notice json should parse");
        assert_eq!(first["type"], "backpressure");
        assert_eq!(first["topic"], "canvas");
        assert!(
            first.get("key").is_none(),
            "an unkeyed topic reports no key"
        );
        assert_eq!(first["dropped_frames"], 1);
        assert_eq!(first["suggested_fps"], 30.0);

        reporter.record_drop(BackpressureAdvice::ReduceFps(60));
        reporter.record_drop(BackpressureAdvice::ReduceFps(60));
        assert!(json_rx.try_recv().is_err());

        let second = tokio::time::timeout(
            BACKPRESSURE_REPORT_INTERVAL + Duration::from_millis(100),
            json_rx.recv(),
        )
        .await
        .expect("pending drops should flush when the report interval elapses")
        .expect("batched notice should send after interval");
        let second: serde_json::Value =
            serde_json::from_str(second.as_str()).expect("second notice json should parse");
        assert_eq!(second["type"], "backpressure");
        assert_eq!(second["topic"], "canvas");
        assert_eq!(second["dropped_frames"], 2);
        assert_eq!(second["suggested_fps"], 30.0);
    }

    #[tokio::test]
    async fn backpressure_reporter_retries_notice_after_queue_drains() {
        let (json_tx, mut json_rx) = mpsc::channel::<Utf8Bytes>(1);
        json_tx
            .try_send("occupied".into())
            .expect("queue accepts its first message");

        let reporter = BackpressureReporter::new(json_tx, "metrics", None);
        reporter.record_drop(BackpressureAdvice::ReduceCadence(
            Cadence::from_fps(0.5).expect("fixture cadence is valid"),
        ));
        tokio::task::yield_now().await;
        assert_eq!(
            json_rx
                .try_recv()
                .expect("occupied message remains")
                .as_str(),
            "occupied"
        );

        let notice = tokio::time::timeout(Duration::from_millis(100), json_rx.recv())
            .await
            .expect("retained notice should resume after capacity returns")
            .expect("retained notice sends after the queue drains");
        let notice: serde_json::Value =
            serde_json::from_str(notice.as_str()).expect("notice json should parse");
        assert_eq!(notice["type"], "backpressure");
        assert_eq!(notice["topic"], "metrics");
        assert_eq!(notice["dropped_frames"], 1);
        assert_eq!(notice["recommendation"], "reduce_fps");
        assert_eq!(notice["suggested_fps"], 0.5);
    }

    #[tokio::test]
    async fn dropping_backpressure_reporter_releases_its_sender() {
        let (json_tx, mut json_rx) = mpsc::channel::<Utf8Bytes>(1);
        let reporter = BackpressureReporter::new(json_tx, "frames", None);

        drop(reporter);

        let closed = tokio::time::timeout(Duration::from_millis(100), json_rx.recv())
            .await
            .expect("reporter task should stop when its owner is dropped");
        assert!(
            closed.is_none(),
            "the reporter task must release its sender"
        );
    }
}
