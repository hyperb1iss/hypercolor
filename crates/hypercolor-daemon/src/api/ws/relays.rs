//! Relay tasks that pump events, frames, spectrum, canvases, and metrics
//! from the render bus out to connected WebSocket clients.
//!
//! Each relay owns its own `tokio::task` and watches an immutable
//! `SubscriptionState` snapshot. Slow consumers are handled with bounded
//! mpsc channels and `try_send` backpressure — drop under load rather than
//! queue unboundedly.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};

use axum::body::Bytes;
use axum::extract::ws::Utf8Bytes;
use hypercolor_types::canvas::PublishedSurfaceStorageIdentity;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{broadcast, watch};
use tracing::{debug, warn};

use super::cache::{
    FrameRelayMessage, WS_CANVAS_BYTES_PER_PIXEL_RGBA, WS_CANVAS_PAYLOAD_BUILD_COUNT,
    WS_CANVAS_PAYLOAD_CACHE_HIT_COUNT, WS_CLIENT_COUNT, WS_FRAME_PAYLOAD_BUILD_COUNT,
    WS_FRAME_PAYLOAD_CACHE_HIT_COUNT, WS_SCREEN_CANVAS_HEADER, WS_TOTAL_BYTES_SENT,
    WS_WEB_VIEWPORT_CANVAS_HEADER, cached_frame_payload, cached_spectrum_payload,
    try_encode_cached_canvas_binary_with_header_scaled, try_encode_cached_canvas_preview_binary,
};
use super::protocol::{
    ActiveFramesConfig, CanvasConfig, MetricsCopies, MetricsDevices, MetricsEffectHealth,
    MetricsFps, MetricsFrameTime, MetricsMemory, MetricsPacing, MetricsPayload, MetricsPreview,
    MetricsPreviewDemand, MetricsRenderSurfaces, MetricsStages, MetricsTimeline, MetricsWebsocket,
    ServerMessage, SpectrumConfig, SubscriptionState, WsChannel, event_message_parts,
    should_relay_event,
};
use crate::api::AppState;
use crate::performance::FrameTimeSummary as RenderFrameTimeSummary;
use crate::preview_runtime::{PreviewDemandSummary, PreviewPixelFormat, PreviewStreamDemand};
use crate::session::OutputPowerState;

const BACKPRESSURE_REPORT_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Default)]
struct BackpressureReporter {
    pending_drops: u32,
    last_reported_at: Option<Instant>,
}

impl BackpressureReporter {
    fn record_drop(
        &mut self,
        json_tx: &tokio::sync::mpsc::Sender<Utf8Bytes>,
        channel: &'static str,
        current_fps: u32,
    ) {
        self.pending_drops = self.pending_drops.saturating_add(1);
        let now = Instant::now();
        let should_report = self.last_reported_at.is_none_or(|last_reported_at| {
            now.saturating_duration_since(last_reported_at) >= BACKPRESSURE_REPORT_INTERVAL
        });
        if !should_report {
            return;
        }

        let dropped_frames = std::mem::take(&mut self.pending_drops);
        self.last_reported_at = Some(now);
        enqueue_backpressure_notice(json_tx, channel, current_fps, dropped_frames);
        debug!(
            channel,
            dropped_frames, current_fps, "Dropping WebSocket binary payloads for slow consumer"
        );
    }
}

/// Relay events from the broadcast bus to a bounded mpsc channel.
/// Drops events when the consumer is slow (backpressure).
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
                    should_relay_event(&timestamped.event, subs.channels)
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

                let _ = try_enqueue_json(&json_tx, json, "events");
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!("WebSocket consumer lagged by {n} events");
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
    let mut backpressure = BackpressureReporter::default();

    loop {
        if active_frame_config.is_none() {
            active_frame_config = {
                let subs = subscriptions.borrow();
                if subs.channels.contains(WsChannel::Frames) {
                    Some(ActiveFramesConfig::new(subs.config.frames.clone()))
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

        match outbound {
            FrameRelayMessage::Json(text) => {
                let _ = try_enqueue_json(&json_tx, text, "frames");
            }
            FrameRelayMessage::Binary(bytes) => {
                if binary_tx.try_send(bytes).is_err() {
                    backpressure.record_drop(&json_tx, "frames", frame_config.config.fps);
                }
            }
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
    let mut backpressure = BackpressureReporter::default();

    loop {
        if active_spectrum_config.is_none() {
            active_spectrum_config = {
                let subs = subscriptions.borrow();
                if subs.channels.contains(WsChannel::Spectrum) {
                    Some(subs.config.spectrum.clone())
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
            backpressure.record_drop(&json_tx, "spectrum", spectrum_config.fps);
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
    json_tx: tokio::sync::mpsc::Sender<Utf8Bytes>,
    binary_tx: tokio::sync::mpsc::Sender<Bytes>,
    mut subscriptions: watch::Receiver<SubscriptionState>,
) {
    let mut canvas_rx = None::<crate::preview_runtime::PreviewFrameReceiver>;
    let mut active_canvas_config = None::<CanvasConfig>;
    let mut receiver_initialized = false;
    let mut last_sent_surface = None::<PreviewSurfaceIdentity>;
    let mut pending_send = false;
    let mut active_fps = 15_u32;
    let mut last_sent_at = preview_initial_last_sent();
    let mut backpressure = BackpressureReporter::default();

    loop {
        if active_canvas_config.is_none() {
            active_canvas_config = {
                let subs = subscriptions.borrow();
                if subs.channels.contains(WsChannel::Canvas) {
                    Some(subs.config.canvas.clone())
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

                if binary_tx.try_send(payload).is_err() {
                    backpressure.record_drop(&json_tx, "canvas", canvas_config.fps);
                    last_sent_at = Instant::now();
                    pending_send = false;
                    continue;
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
    json_tx: tokio::sync::mpsc::Sender<Utf8Bytes>,
    binary_tx: tokio::sync::mpsc::Sender<Bytes>,
    mut subscriptions: watch::Receiver<SubscriptionState>,
) {
    let mut canvas_rx = None::<crate::preview_runtime::PreviewFrameReceiver>;
    let mut active_canvas_config = None::<CanvasConfig>;
    let mut receiver_initialized = false;
    let mut last_sent_surface = None::<PreviewSurfaceIdentity>;
    let mut pending_send = false;
    let mut active_fps = 15_u32;
    let mut last_sent_at = preview_initial_last_sent();
    let mut backpressure = BackpressureReporter::default();

    loop {
        if active_canvas_config.is_none() {
            active_canvas_config = {
                let subs = subscriptions.borrow();
                if subs.channels.contains(WsChannel::ScreenCanvas) {
                    Some(subs.config.screen_canvas.clone())
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

                if binary_tx.try_send(payload).is_err() {
                    backpressure.record_drop(&json_tx, "screen_canvas", canvas_config.fps);
                    last_sent_at = Instant::now();
                    pending_send = false;
                    continue;
                }

                last_sent_at = Instant::now();
                last_sent_surface = Some(surface_identity);
                pending_send = false;
            }
        }
    }
}

pub(super) async fn relay_web_viewport_canvas(
    preview_runtime: Arc<crate::preview_runtime::PreviewRuntime>,
    json_tx: tokio::sync::mpsc::Sender<Utf8Bytes>,
    binary_tx: tokio::sync::mpsc::Sender<Bytes>,
    mut subscriptions: watch::Receiver<SubscriptionState>,
) {
    let mut canvas_rx = None::<crate::preview_runtime::PreviewFrameReceiver>;
    let mut active_canvas_config = None::<CanvasConfig>;
    let mut receiver_initialized = false;
    let mut last_sent_surface = None::<PreviewSurfaceIdentity>;
    let mut pending_send = false;
    let mut active_fps = 15_u32;
    let mut last_sent_at = preview_initial_last_sent();
    let mut backpressure = BackpressureReporter::default();

    loop {
        if active_canvas_config.is_none() {
            active_canvas_config = {
                let subs = subscriptions.borrow();
                if subs.channels.contains(WsChannel::WebViewportCanvas) {
                    Some(subs.config.web_viewport_canvas.clone())
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

                if binary_tx.try_send(payload).is_err() {
                    backpressure.record_drop(
                        &json_tx,
                        "web_viewport_canvas",
                        canvas_config.fps,
                    );
                    last_sent_at = Instant::now();
                    pending_send = false;
                    continue;
                }

                last_sent_at = Instant::now();
                last_sent_surface = Some(surface_identity);
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

/// Relay composited display-preview JPEG frames for a client's selected
/// display. Unlike the canvas/screen-canvas relays, this one is
/// parameterized by `device_id` — switching the config's `device_id`
/// detaches the old watch subscriber and attaches a fresh one for the
/// new display.
///
/// Pacing mirrors `relay_canvas`: the sleep branch is guarded by
/// `pending_send` so the task never tight-loops when nothing has
/// changed. `dead_target_id` tracks the last device observed as gone so
/// we don't auto-resubscribe to a silent channel until the client sends
/// a new subscription — without this, a removed device would spin the
/// registry write lock once per loop iteration.
pub(super) async fn relay_display_preview(
    display_frames: Arc<tokio::sync::RwLock<crate::display_frames::DisplayFrameRuntime>>,
    json_tx: tokio::sync::mpsc::Sender<Utf8Bytes>,
    binary_tx: tokio::sync::mpsc::Sender<Bytes>,
    mut subscriptions: watch::Receiver<SubscriptionState>,
) {
    use crate::display_frames::DisplayFrameSnapshot;
    use hypercolor_types::device::DeviceId;
    use std::str::FromStr;

    /// Target the relay is currently following: which device, at what
    /// fps, with a live watch receiver. Rebuilt whenever the client's
    /// device_id changes or the channel goes idle.
    struct ActiveTarget {
        device_id: DeviceId,
        fps: u32,
        rx: watch::Receiver<Option<Arc<DisplayFrameSnapshot>>>,
        last_frame_number: Option<u64>,
        last_sent_at: Instant,
        pending_send: bool,
    }

    let mut active: Option<ActiveTarget> = None;
    let mut dead_target_id: Option<DeviceId> = None;
    let mut backpressure = BackpressureReporter::default();

    loop {
        // Re-derive the desired target from the current subscription
        // state. We skip retargeting when the desired id matches a
        // device we've already observed as gone — the client must send
        // a fresh subscribe (possibly for the same id, after reconnect)
        // to re-arm the relay.
        let desired = {
            let subs = subscriptions.borrow();
            if subs.channels.contains(WsChannel::DisplayPreview) {
                subs.config
                    .display_preview
                    .device_id
                    .as_ref()
                    .and_then(|raw| DeviceId::from_str(raw).ok())
                    .map(|id| (id, subs.config.display_preview.fps.max(1)))
            } else {
                None
            }
        };

        match (&active, desired) {
            (None, None) => {}
            (Some(current), Some((want_id, want_fps))) if current.device_id == want_id => {
                if current.fps != want_fps
                    && let Some(target) = active.as_mut()
                {
                    target.fps = want_fps;
                }
            }
            (_, None) => {
                active = None;
                dead_target_id = None;
            }
            (_, Some((want_id, _))) if dead_target_id == Some(want_id) => {
                // Avoid spin-resubscribing to a removed device. Wait
                // for the next subscription change before trying again.
                active = None;
            }
            (_, Some((want_id, want_fps))) => {
                let rx = display_frames.write().await.subscribe(want_id);
                // `watch::Sender::subscribe()` marks the new receiver as
                // already-observed, so rx.changed() will not fire for the
                // initial value. Prime pending_send when a snapshot exists
                // so the first sleep tick delivers the current frame —
                // otherwise clients would stall on connect until the
                // daemon publishes a fresh frame.
                let has_initial_frame = rx.borrow().is_some();
                active = Some(ActiveTarget {
                    device_id: want_id,
                    fps: want_fps,
                    rx,
                    last_frame_number: None,
                    last_sent_at: preview_initial_last_sent(),
                    pending_send: has_initial_frame,
                });
            }
        }

        let Some(target) = active.as_mut() else {
            if subscriptions.changed().await.is_err() {
                break;
            }
            let _ = subscriptions.borrow_and_update();
            // Any subscription change clears the dead-target latch so
            // the client can retarget the same (possibly reconnected)
            // device without first bouncing to a different id.
            dead_target_id = None;
            continue;
        };

        tokio::select! {
            changed = target.rx.changed() => {
                if changed.is_err() {
                    // Sender dropped — device gone. Latch the id so we
                    // don't auto-resubscribe until the subscription
                    // changes, and clear active.
                    dead_target_id = Some(target.device_id);
                    active = None;
                    continue;
                }
                // Either a new frame or the terminal None marker.
                // Inspect after the select to decide what to do.
                target.pending_send = true;
            }
            changed = subscriptions.changed() => {
                if changed.is_err() {
                    break;
                }
                let _ = subscriptions.borrow_and_update();
                dead_target_id = None;
                continue;
            }
            () = tokio::time::sleep(preview_send_delay(target.last_sent_at, target.fps, Instant::now())), if target.pending_send => {
                let snapshot = target.rx.borrow().as_ref().map(Arc::clone);
                let Some(snapshot) = snapshot else {
                    // Sender signaled device removal via None. Clear
                    // pending_send to avoid re-entering this branch
                    // immediately; the next rx.changed() or subscription
                    // change will re-arm us.
                    dead_target_id = Some(target.device_id);
                    active = None;
                    continue;
                };

                if target.last_frame_number == Some(snapshot.frame_number) {
                    // No forward motion since last send — nothing to do.
                    target.pending_send = false;
                    continue;
                }

                let payload = encode_display_preview_frame(&snapshot);
                if binary_tx.try_send(payload).is_err() {
                    backpressure.record_drop(&json_tx, "display_preview", target.fps);
                    // Advance last_sent_at so the next retry waits out a
                    // full fps interval instead of spinning the encoder.
                    // Clear pending_send too — if the consumer is slow
                    // enough to fill the queue, a fresh rx.changed() will
                    // re-arm us for whichever frame is current then.
                    target.last_sent_at = Instant::now();
                    target.pending_send = false;
                    continue;
                }

                target.last_frame_number = Some(snapshot.frame_number);
                target.last_sent_at = Instant::now();
                target.pending_send = false;
            }
        }
    }
}

/// Serialize a display-preview snapshot into its WebSocket binary payload.
///
/// Layout (little-endian where noted):
/// `[0x07: u8][frame_number: u32][timestamp_ms: u32][width: u16][height: u16][format: u8 = 2 (JPEG)][jpeg_payload...]`
fn encode_display_preview_frame(snapshot: &crate::display_frames::DisplayFrameSnapshot) -> Bytes {
    const JPEG_FORMAT: u8 = 2;
    const HEADER_LEN: usize = 1 + 4 + 4 + 2 + 2 + 1;

    let jpeg = snapshot.jpeg_data.as_ref().as_slice();
    let mut buf = Vec::with_capacity(HEADER_LEN + jpeg.len());
    buf.push(super::cache::WS_DISPLAY_PREVIEW_HEADER);

    // Frame number truncates cleanly for the common case (< 2^32 frames);
    // on overflow it wraps, which is fine since consumers only use it for
    // change detection, not absolute positioning.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "u64 → u32 truncation is intentional; frame number wraps for change detection only"
    )]
    let frame_u32 = snapshot.frame_number as u32;
    buf.extend_from_slice(&frame_u32.to_le_bytes());

    let timestamp_ms = snapshot
        .captured_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "timestamp truncates to u32 millis; consumers use it as a monotonic hint, not wall clock"
    )]
    let timestamp_u32 = timestamp_ms as u32;
    buf.extend_from_slice(&timestamp_u32.to_le_bytes());

    let width_u16 = u16::try_from(snapshot.width).unwrap_or(u16::MAX);
    let height_u16 = u16::try_from(snapshot.height).unwrap_or(u16::MAX);
    buf.extend_from_slice(&width_u16.to_le_bytes());
    buf.extend_from_slice(&height_u16.to_le_bytes());
    buf.push(JPEG_FORMAT);
    buf.extend_from_slice(jpeg);

    Bytes::from(buf)
}

fn preview_stream_demand(config: &CanvasConfig) -> PreviewStreamDemand {
    PreviewStreamDemand {
        fps: config.fps,
        format: match config.format {
            super::protocol::CanvasFormat::Rgb => PreviewPixelFormat::Rgb,
            super::protocol::CanvasFormat::Rgba => PreviewPixelFormat::Rgba,
            super::protocol::CanvasFormat::Jpeg => PreviewPixelFormat::Jpeg,
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
    let mut active_interval_ms = None::<u32>;

    loop {
        if active_interval_ms.is_none() {
            active_interval_ms = {
                let subs = subscriptions.borrow();
                if subs.channels.contains(WsChannel::Metrics) {
                    Some(subs.config.metrics.interval_ms)
                } else {
                    None
                }
            };
        }

        let Some(interval_ms) = active_interval_ms else {
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
                active_interval_ms = None;
                continue;
            }
            () = tokio::time::sleep(Duration::from_millis(u64::from(interval_ms))) => {}
        }

        let still_subscribed = {
            let subs = subscriptions.borrow();
            subs.channels.contains(WsChannel::Metrics)
        };
        if !still_subscribed {
            continue;
        }

        let total_bytes = WS_TOTAL_BYTES_SENT.load(Ordering::Relaxed);
        let delta_bytes = total_bytes.saturating_sub(last_total_bytes);
        last_total_bytes = total_bytes;
        let interval_secs = f64::from(interval_ms) / 1000.0;
        let bytes_per_sec = if interval_secs > 0.0 {
            let delta_u32 = u32::try_from(delta_bytes).unwrap_or(u32::MAX);
            f64::from(delta_u32) / interval_secs
        } else {
            0.0
        };

        let message = build_metrics_message(&state, bytes_per_sec).await;
        if let Ok(text) = serde_json::to_string(&message) {
            let _ = try_enqueue_json(&json_tx, text, "metrics");
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
                debug!(
                    stream,
                    "Dropping queued WebSocket JSON message for slow consumer"
                );
            }
            false
        }
        Err(TrySendError::Closed(_)) => false,
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

fn enqueue_backpressure_notice(
    json_tx: &tokio::sync::mpsc::Sender<Utf8Bytes>,
    channel: &str,
    current_fps: u32,
    dropped_frames: u32,
) {
    let suggested_fps = current_fps.saturating_div(2).max(1);
    let message = ServerMessage::Backpressure {
        dropped_frames: dropped_frames.max(1),
        channel: channel.to_owned(),
        recommendation: "reduce_fps".to_owned(),
        suggested_fps,
    };

    if let Ok(text) = serde_json::to_string(&message) {
        let _ = try_enqueue_json(json_tx, text, "backpressure");
    }
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
    let target_fps = render_stats.tier.fps();
    let ceiling_fps = render_stats.max_tier.fps();
    let avg_frame_secs = render_stats.avg_frame_time.as_secs_f64();
    let actual_fps = paced_fps(avg_frame_secs, target_fps);
    let avg_ms = avg_frame_secs * 1000.0;
    let frame_time = frame_time_summary(performance_snapshot.frame_time, avg_ms);
    let latest_frame = performance_snapshot.latest_frame.unwrap_or_default();
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
    let screen_canvas_demand = state.preview_runtime.screen_canvas_demand();
    let web_viewport_canvas_demand = state.preview_runtime.web_viewport_canvas_demand();
    let (servo_soft_stalls_total, servo_breaker_opens_total) = servo_effect_health_counts();

    ServerMessage::Metrics {
        timestamp: format_iso8601_now(),
        data: MetricsPayload {
            fps: MetricsFps {
                target: target_fps,
                ceiling: ceiling_fps,
                actual: round_1(actual_fps),
                dropped: render_stats.consecutive_misses,
            },
            frame_time: MetricsFrameTime {
                avg_ms: round_2(frame_time.avg_ms),
                p95_ms: round_2(frame_time.p95_ms),
                p99_ms: round_2(frame_time.p99_ms),
                max_ms: round_2(frame_time.max_ms),
            },
            stages: MetricsStages {
                input_sampling_ms: round_2(us_to_ms(latest_frame.input_us)),
                producer_rendering_ms: round_2(us_to_ms(latest_frame.producer_us)),
                producer_effect_rendering_ms: round_2(us_to_ms(latest_frame.producer_render_us)),
                producer_preview_compose_ms: round_2(us_to_ms(
                    latest_frame.producer_preview_compose_us,
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
                frame_age_ms: round_2(frame_age_ms),
                reused_inputs: performance_snapshot.pacing.reused_inputs,
                reused_canvas: performance_snapshot.pacing.reused_canvas,
                retained_effect: performance_snapshot.pacing.retained_effect,
                retained_screen: performance_snapshot.pacing.retained_screen,
                composition_bypassed: performance_snapshot.pacing.composition_bypassed,
                gpu_zone_sampling: performance_snapshot.pacing.gpu_zone_sampling,
                gpu_sample_deferred: performance_snapshot.pacing.gpu_sample_deferred,
                gpu_sample_retry_hit: performance_snapshot.pacing.gpu_sample_retry_hit,
                gpu_sample_queue_saturated: performance_snapshot.pacing.gpu_sample_queue_saturated,
                gpu_sample_wait_blocked: performance_snapshot.pacing.gpu_sample_wait_blocked,
            },
            effect_health: MetricsEffectHealth {
                errors_total: performance_snapshot.effect_health.errors_total,
                fallbacks_applied_total: performance_snapshot.effect_health.fallbacks_applied_total,
                servo_soft_stalls_total,
                servo_breaker_opens_total,
            },
            timeline: MetricsTimeline {
                frame_token: latest_frame.timeline.frame_token,
                compositor_backend: latest_frame.compositor_backend.as_str().to_owned(),
                gpu_zone_sampling: latest_frame.gpu_zone_sampling,
                gpu_sample_deferred: latest_frame.gpu_sample_deferred,
                gpu_sample_retry_hit: latest_frame.gpu_sample_retry_hit,
                gpu_sample_queue_saturated: latest_frame.gpu_sample_queue_saturated,
                gpu_sample_wait_blocked: latest_frame.gpu_sample_wait_blocked,
                cpu_readback_skipped: latest_frame.cpu_readback_skipped,
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
                producer_done_ms: round_2(us_to_ms(latest_frame.timeline.producer_done_us)),
                composition_done_ms: round_2(us_to_ms(latest_frame.timeline.composition_done_us)),
                sampling_done_ms: round_2(us_to_ms(latest_frame.timeline.sample_done_us)),
                output_done_ms: round_2(us_to_ms(latest_frame.timeline.output_done_us)),
                publish_done_ms: round_2(us_to_ms(latest_frame.timeline.publish_done_us)),
                frame_done_ms: round_2(us_to_ms(latest_frame.timeline.frame_done_us)),
            },
            render_surfaces: MetricsRenderSurfaces {
                slot_count: latest_frame.render_surface_slot_count,
                free_slots: latest_frame.render_surface_free_slots,
                published_slots: latest_frame.render_surface_published_slots,
                dequeued_slots: latest_frame.render_surface_dequeued_slots,
                canvas_receivers: latest_frame.canvas_receiver_count,
                preview_pool_saturation_reallocs: latest_frame.preview_pool_saturation_reallocs,
                direct_pool_saturation_reallocs: latest_frame.direct_pool_saturation_reallocs,
                preview_pool_grown_slots: latest_frame.preview_pool_grown_slots,
                direct_pool_grown_slots: latest_frame.direct_pool_grown_slots,
            },
            preview: MetricsPreview {
                canvas_receivers: preview_runtime.canvas_receivers,
                screen_canvas_receivers: preview_runtime.screen_canvas_receivers,
                web_viewport_canvas_receivers: preview_runtime.web_viewport_canvas_receivers,
                canvas_frames_published: preview_runtime.canvas_frames_published,
                screen_canvas_frames_published: preview_runtime.screen_canvas_frames_published,
                web_viewport_canvas_frames_published: preview_runtime
                    .web_viewport_canvas_frames_published,
                latest_canvas_frame_number: preview_runtime.latest_canvas_frame_number,
                latest_screen_canvas_frame_number: preview_runtime
                    .latest_screen_canvas_frame_number,
                latest_web_viewport_canvas_frame_number: preview_runtime
                    .latest_web_viewport_canvas_frame_number,
                canvas_demand: metrics_preview_demand(canvas_demand),
                screen_canvas_demand: metrics_preview_demand(screen_canvas_demand),
                web_viewport_canvas_demand: metrics_preview_demand(web_viewport_canvas_demand),
            },
            copies: MetricsCopies {
                full_frame_count: latest_frame.full_frame_copy_count,
                full_frame_kb: round_2(bytes_to_kib(latest_frame.full_frame_copy_bytes)),
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
            },
        },
    }
}

#[cfg(feature = "servo")]
fn servo_effect_health_counts() -> (u64, u64) {
    let snapshot = hypercolor_core::effect::servo_telemetry_snapshot();
    (snapshot.soft_stalls_total, snapshot.breaker_opens_total)
}

#[cfg(not(feature = "servo"))]
const fn servo_effect_health_counts() -> (u64, u64) {
    (0, 0)
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
mod tests {
    use std::time::{Duration, Instant};

    use axum::extract::ws::Utf8Bytes;
    use hypercolor_core::bus::CanvasFrame;
    use hypercolor_types::canvas::{Canvas, PublishedSurface};
    use tokio::sync::mpsc;

    use super::{BackpressureReporter, preview_send_delay, preview_surface_identity};

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
        let mut reporter = BackpressureReporter::default();

        reporter.record_drop(&json_tx, "canvas", 60);
        let first = json_rx
            .try_recv()
            .expect("first notice should send immediately");
        let first: serde_json::Value =
            serde_json::from_str(first.as_str()).expect("first notice json should parse");
        assert_eq!(first["type"], "backpressure");
        assert_eq!(first["channel"], "canvas");
        assert_eq!(first["dropped_frames"], 1);
        assert_eq!(first["suggested_fps"], 30);

        reporter.record_drop(&json_tx, "canvas", 60);
        assert!(json_rx.try_recv().is_err());

        reporter.last_reported_at = Some(
            Instant::now()
                .checked_sub(Duration::from_secs(1))
                .unwrap_or_else(Instant::now),
        );
        reporter.record_drop(&json_tx, "canvas", 60);

        let second = json_rx
            .try_recv()
            .expect("batched notice should send after interval");
        let second: serde_json::Value =
            serde_json::from_str(second.as_str()).expect("second notice json should parse");
        assert_eq!(second["type"], "backpressure");
        assert_eq!(second["channel"], "canvas");
        assert_eq!(second["dropped_frames"], 2);
        assert_eq!(second["suggested_fps"], 30);
    }
}
