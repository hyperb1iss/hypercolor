//! Per-device display worker event loop.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::anyhow;
use tokio::sync::{RwLock, watch};
use tokio::task::JoinHandle;
use tracing::{trace, warn};

use hypercolor_core::bus::CanvasFrame;
use hypercolor_core::device::{BackendIo, DeviceDisplaySink};
use hypercolor_types::device::{DeviceId, DisplayFrameFormat, OwnedDisplayFramePayload};
use hypercolor_types::session::OffOutputBehavior;

use super::encode::{
    DisplayEncodeState, display_brightness_factor, encode_canvas_frame, encode_face_scene_blend,
};
use super::render::display_viewport_signature;
use super::{
    DISPLAY_ERROR_WARN_INTERVAL, DisplayGeometry, DisplaySourceIdentity, DisplayTarget,
    DisplayViewportSignature, DisplayWorkerConfigSignature, DisplayWorkerFrameSet,
    DisplayWorkerFrameSource,
};
use crate::deadline::advance_deadline;
use crate::display_frames::{DisplayFrameRuntime, DisplayFrameSnapshot};
use crate::session::OutputPowerState;

const DISPLAY_SINK_LOOKUP_RETRY_INTERVAL: Duration = Duration::from_millis(250);

async fn publish_display_frame_snapshot(
    display_frames: &Arc<RwLock<DisplayFrameRuntime>>,
    device_id: DeviceId,
    geometry: &DisplayGeometry,
    frame_number: u64,
    jpeg: Arc<Vec<u8>>,
) {
    display_frames.write().await.set_frame(
        device_id,
        DisplayFrameSnapshot {
            jpeg_data: jpeg,
            width: geometry.width,
            height: geometry.height,
            circular: geometry.circular,
            frame_number,
            captured_at: SystemTime::now(),
        },
    );
}

fn preview_jpeg_for_payload(
    payload: &Arc<OwnedDisplayFramePayload>,
    raw_preview: Option<Arc<Vec<u8>>>,
) -> Option<Arc<Vec<u8>>> {
    match payload.format {
        DisplayFrameFormat::Jpeg => Some(Arc::clone(&payload.data)),
        DisplayFrameFormat::Rgb => raw_preview,
    }
}

pub(super) struct DisplayWorkerHandle {
    tx: watch::Sender<Option<DisplayWorkerFrameSet>>,
    join_handle: JoinHandle<()>,
    pub config_signature: DisplayWorkerConfigSignature,
}

struct DisplayDeviceWriter {
    backend_io: BackendIo,
    display_sink: Option<Arc<dyn DeviceDisplaySink>>,
    next_display_sink_lookup_at: Option<Instant>,
}

impl DisplayDeviceWriter {
    const fn new(backend_io: BackendIo, display_sink: Option<Arc<dyn DeviceDisplaySink>>) -> Self {
        Self {
            backend_io,
            display_sink,
            next_display_sink_lookup_at: None,
        }
    }

    async fn write_display_payload_owned(
        &mut self,
        device_id: DeviceId,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> anyhow::Result<()> {
        let now = Instant::now();
        if self.display_sink.is_none()
            && self
                .next_display_sink_lookup_at
                .is_none_or(|retry_at| now >= retry_at)
        {
            self.display_sink = self.backend_io.display_sink(device_id).await;
            self.next_display_sink_lookup_at = self
                .display_sink
                .is_none()
                .then_some(now + DISPLAY_SINK_LOOKUP_RETRY_INTERVAL);
        }

        if let Some(sink) = self.display_sink.as_ref() {
            if let Err(error) = sink.write_display_payload_owned(Arc::clone(&payload)).await {
                self.display_sink = None;
                self.next_display_sink_lookup_at = None;
                return Err(error);
            }
            return Ok(());
        }

        self.backend_io
            .write_display_payload_owned(device_id, payload)
            .await
    }
}

#[derive(Clone)]
enum PendingDisplayFrame {
    Fresh(DisplayWorkerFrameSet),
    StaticHold,
    RetryAfterFailure(DisplayWorkerFrameSet),
}

impl PendingDisplayFrame {
    fn fresh(frames: DisplayWorkerFrameSet) -> Self {
        Self::Fresh(frames)
    }

    fn retry(frames: DisplayWorkerFrameSet) -> Self {
        Self::RetryAfterFailure(frames)
    }

    const fn force_send(&self) -> bool {
        !matches!(self, Self::Fresh(_))
    }

    const fn is_retry(&self) -> bool {
        matches!(self, Self::RetryAfterFailure(_))
    }

    fn into_frames(self) -> Option<DisplayWorkerFrameSet> {
        match self {
            Self::Fresh(frames) | Self::RetryAfterFailure(frames) => Some(frames),
            Self::StaticHold => None,
        }
    }
}

#[derive(Clone)]
struct CapturedDisplaySource {
    identity: DisplaySourceIdentity,
    content_hash: Option<u64>,
}

#[derive(Clone)]
enum CapturedDisplayFrameSource {
    Scene(CapturedDisplaySource),
    Face {
        scene_source: Option<CapturedDisplaySource>,
        face_source: CapturedDisplaySource,
        blend_mode: hypercolor_types::scene::DisplayFaceBlendMode,
        opacity_bits: u32,
    },
}

#[derive(Clone)]
struct DisplayFrameInputState {
    source: CapturedDisplayFrameSource,
    brightness_factor: u16,
    geometry: DisplayGeometry,
    viewport: DisplayViewportSignature,
}

impl CapturedDisplayFrameSource {
    fn matches(&self, source: &DisplayWorkerFrameSource) -> bool {
        match (self, source) {
            (Self::Scene(captured), DisplayWorkerFrameSource::Scene(frame)) => {
                display_source_matches(Some(captured), Some(frame))
            }
            (
                Self::Face {
                    scene_source,
                    face_source,
                    blend_mode,
                    opacity_bits,
                },
                DisplayWorkerFrameSource::Face {
                    scene_frame,
                    face_frame,
                    blend_mode: current_blend_mode,
                    opacity,
                },
            ) => {
                display_source_matches(scene_source.as_ref(), scene_frame.as_ref())
                    && display_source_matches(Some(face_source), Some(face_frame))
                    && blend_mode == current_blend_mode
                    && *opacity_bits == opacity.to_bits()
            }
            _ => false,
        }
    }

    fn capture(source: &DisplayWorkerFrameSource) -> Self {
        match source {
            DisplayWorkerFrameSource::Scene(frame) => Self::Scene(
                capture_display_source(Some(frame))
                    .expect("display worker should only capture a valid scene frame"),
            ),
            DisplayWorkerFrameSource::Face {
                scene_frame,
                face_frame,
                blend_mode,
                opacity,
            } => Self::Face {
                scene_source: capture_display_source(scene_frame.as_ref()),
                face_source: capture_display_source(Some(face_frame))
                    .expect("display worker should only capture a valid face frame"),
                blend_mode: *blend_mode,
                opacity_bits: opacity.to_bits(),
            },
        }
    }
}

impl DisplayFrameInputState {
    fn matches(&self, frames: &DisplayWorkerFrameSet, target: &DisplayTarget) -> bool {
        self.source.matches(&frames.source)
            && self.brightness_factor == display_brightness_factor(target.brightness)
            && self.geometry == target.geometry
            && self.viewport == display_viewport_signature(&target.viewport)
    }

    fn capture(frames: &DisplayWorkerFrameSet, target: &DisplayTarget) -> Self {
        Self {
            source: CapturedDisplayFrameSource::capture(&frames.source),
            brightness_factor: display_brightness_factor(target.brightness),
            geometry: target.geometry,
            viewport: display_viewport_signature(&target.viewport),
        }
    }
}

fn display_source_matches(
    captured: Option<&CapturedDisplaySource>,
    source: Option<&Arc<CanvasFrame>>,
) -> bool {
    match (captured, source) {
        (None, None) => true,
        (Some(captured), Some(source)) => {
            let source_identity = display_source_identity(source.as_ref());
            captured.identity == source_identity || display_source_content_matches(captured, source)
        }
        _ => false,
    }
}

fn capture_display_source(source: Option<&Arc<CanvasFrame>>) -> Option<CapturedDisplaySource> {
    source.map(|source| {
        let identity = display_source_identity(source.as_ref());
        CapturedDisplaySource {
            identity,
            content_hash: should_hash_display_source_identity(identity)
                .then(|| display_source_content_hash(source.as_ref())),
        }
    })
}

fn display_source_identity(source: &CanvasFrame) -> DisplaySourceIdentity {
    DisplaySourceIdentity {
        generation: source.surface().generation(),
        storage: source.surface().storage_identity(),
        width: source.width,
        height: source.height,
    }
}

fn display_source_content_matches(captured: &CapturedDisplaySource, source: &CanvasFrame) -> bool {
    if !should_hash_display_source_identity(captured.identity) {
        return false;
    }

    let Some(captured_hash) = captured.content_hash else {
        return false;
    };
    let source_identity = display_source_identity(source);
    captured.identity.width == source_identity.width
        && captured.identity.height == source_identity.height
        && should_hash_display_source_identity(source_identity)
        && captured_hash == display_source_content_hash(source)
}

fn should_hash_display_source_identity(identity: DisplaySourceIdentity) -> bool {
    identity.generation == 0
}

fn display_source_content_hash(source: &CanvasFrame) -> u64 {
    let mut hasher = DefaultHasher::new();
    source.width.hash(&mut hasher);
    source.height.hash(&mut hasher);
    source.rgba_bytes().hash(&mut hasher);
    hasher.finish()
}

impl DisplayWorkerHandle {
    #[allow(
        clippy::too_many_arguments,
        reason = "worker spawn plumbs every shared subsystem it consumes"
    )]
    pub fn spawn(
        target: Arc<DisplayTarget>,
        backend_io: BackendIo,
        display_sink: Option<Arc<dyn DeviceDisplaySink>>,
        power_state: watch::Receiver<OutputPowerState>,
        static_hold_refresh_interval: Duration,
        display_frames: Arc<RwLock<DisplayFrameRuntime>>,
    ) -> Self {
        let (tx, rx) = watch::channel(None::<DisplayWorkerFrameSet>);
        let worker_backend_id = target.backend_id.clone();
        let worker_device_id = target.device_id;
        let config_signature = target.worker_config_signature();
        let writer = DisplayDeviceWriter::new(backend_io, display_sink);
        let join_handle = tokio::spawn(run_display_worker(
            writer,
            worker_backend_id,
            worker_device_id,
            target.as_ref().clone(),
            rx,
            power_state,
            static_hold_refresh_interval,
            display_frames,
        ));

        Self {
            tx,
            join_handle,
            config_signature,
        }
    }

    pub fn push(&self, frames: DisplayWorkerFrameSet) {
        self.tx.send_replace(Some(frames));
    }

    pub async fn shutdown(self) {
        drop(self.tx);
        let _ = self.join_handle.await;
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "display worker is a self-contained event loop"
)]
#[allow(
    clippy::too_many_arguments,
    reason = "display worker borrows every subsystem it drives"
)]
async fn run_display_worker(
    mut writer: DisplayDeviceWriter,
    backend_key: String,
    device_id: DeviceId,
    target: DisplayTarget,
    mut rx: watch::Receiver<Option<DisplayWorkerFrameSet>>,
    mut power_state: watch::Receiver<OutputPowerState>,
    static_hold_refresh_interval: Duration,
    display_frames: Arc<RwLock<DisplayFrameRuntime>>,
) {
    let target_fps = target.target_fps;
    let send_interval = target_interval_for_fps(target_fps);
    let mut next_send_at = Instant::now();
    let mut last_warned_at = None::<Instant>;
    let mut last_delivered_input = None::<DisplayFrameInputState>;
    let mut next_hold_refresh_at = None::<Instant>;
    let mut encode_state = match DisplayEncodeState::new() {
        Ok(state) => state,
        Err(error) => {
            warn!(
                backend_id = %backend_key,
                device_id = %device_id,
                error = %error,
                "display worker failed to initialize encoder state"
            );
            return;
        }
    };
    let mut pending = None::<PendingDisplayFrame>;
    let mut retry_after = None::<Instant>;
    let mut delivered_frame_number = 0_u64;
    // Monotonic per-worker counter incremented on every preview publish so
    // repeated write failures don't reuse the same ETag for different JPEGs.
    // Decoupled from delivered_frame_number, which only advances on
    // successful device writes.
    let mut preview_frame_number = 0_u64;
    let mut last_delivered_payload = None::<Arc<OwnedDisplayFramePayload>>;
    let mut last_delivered_preview_jpeg = None::<Arc<Vec<u8>>>;

    'worker: loop {
        if pending.is_none() {
            if let Some(wake_deadline) = next_hold_refresh_at {
                tokio::select! {
                    changed = rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        pending = rx.borrow_and_update().clone().map(PendingDisplayFrame::fresh);
                        retry_after = None;
                    }
                    changed = power_state.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let _ = power_state.borrow_and_update();
                    }
                        () = tokio::time::sleep_until(tokio::time::Instant::from_std(wake_deadline)) => {
                        if should_refresh_static_hold(&power_state) && last_delivered_payload.is_some() {
                            pending = Some(PendingDisplayFrame::StaticHold);
                        }
                    }
                }
            } else {
                tokio::select! {
                    changed = rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        pending = rx.borrow_and_update().clone().map(PendingDisplayFrame::fresh);
                        retry_after = None;
                    }
                    changed = power_state.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let _ = power_state.borrow_and_update();
                    }
                }
            }
            next_hold_refresh_at = static_hold_refresh_deadline(
                &power_state,
                last_delivered_payload.is_some(),
                static_hold_refresh_interval,
            );
            continue;
        }

        if let Some(retry_deadline) = retry_after.take() {
            tokio::select! {
                changed = rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    pending = rx.borrow_and_update().clone().map(PendingDisplayFrame::fresh);
                    continue;
                }
                () = tokio::time::sleep_until(tokio::time::Instant::from_std(retry_deadline)) => {}
            }
        }

        if send_interval.is_some() {
            while Instant::now() < next_send_at {
                tokio::select! {
                    changed = rx.changed() => {
                        if changed.is_err() {
                            break 'worker;
                        }
                        pending = rx.borrow_and_update().clone().map(PendingDisplayFrame::fresh);
                        retry_after = None;
                        if pending.is_none() {
                            continue 'worker;
                        }
                    }
                    () = tokio::time::sleep_until(tokio::time::Instant::from_std(next_send_at)) => {
                        break;
                    }
                }
            }
        }

        let Some(pending_frame) = pending.take() else {
            continue;
        };
        let force_send = pending_frame.force_send();
        let retry_attempt = pending_frame.is_retry();

        let Some(frames) = pending_frame.into_frames() else {
            if let Some(payload) = last_delivered_payload.as_ref() {
                if let Some(preview_jpeg) =
                    preview_jpeg_for_payload(payload, last_delivered_preview_jpeg.clone())
                {
                    preview_frame_number = preview_frame_number.saturating_add(1);
                    publish_display_frame_snapshot(
                        &display_frames,
                        device_id,
                        &target.geometry,
                        preview_frame_number,
                        preview_jpeg,
                    )
                    .await;
                }
                record_display_write_attempt(&display_frames, false).await;
                let write_result = writer
                    .write_display_payload_owned(device_id, Arc::clone(payload))
                    .await;
                if let Err(error) = write_result {
                    record_display_write_failure(&display_frames).await;
                    maybe_warn_display_error(&mut last_warned_at, &target, &error);
                } else {
                    record_display_write_success(&display_frames).await;
                    delivered_frame_number = delivered_frame_number.saturating_add(1);
                }
            }
            next_hold_refresh_at = static_hold_refresh_deadline(
                &power_state,
                last_delivered_payload.is_some(),
                static_hold_refresh_interval,
            );
            continue;
        };

        let zero_brightness_output = display_brightness_factor(target.brightness) == 0;
        let input_matches = last_delivered_input
            .as_ref()
            .is_some_and(|previous| previous.matches(&frames, &target));
        if zero_brightness_output && !force_send && last_delivered_payload.is_some() {
            trace!(
                device = %target.name,
                backend_id = %backend_key,
                device_id = %device_id,
                target_fps,
                "skipping zero-brightness display frame"
            );
            continue;
        }
        if input_matches && !force_send {
            trace!(
                device = %target.name,
                backend_id = %backend_key,
                device_id = %device_id,
                target_fps,
                "skipping unchanged display frame"
            );
            continue;
        }
        if force_send
            && (input_matches || zero_brightness_output)
            && let Some(payload) = last_delivered_payload.as_ref()
        {
            if let Some(preview_jpeg) =
                preview_jpeg_for_payload(payload, last_delivered_preview_jpeg.clone())
            {
                preview_frame_number = preview_frame_number.saturating_add(1);
                publish_display_frame_snapshot(
                    &display_frames,
                    device_id,
                    &target.geometry,
                    preview_frame_number,
                    preview_jpeg,
                )
                .await;
            }
            record_display_write_attempt(&display_frames, retry_attempt).await;
            let write_result = writer
                .write_display_payload_owned(device_id, Arc::clone(payload))
                .await;
            if let Err(error) = write_result {
                record_display_write_failure(&display_frames).await;
                maybe_warn_display_error(&mut last_warned_at, &target, &error);
                schedule_display_retry(
                    &mut pending,
                    &mut retry_after,
                    &mut next_send_at,
                    send_interval,
                    static_hold_refresh_interval,
                    frames,
                );
                continue;
            }
            record_display_write_success(&display_frames).await;
            next_hold_refresh_at = static_hold_refresh_deadline(
                &power_state,
                last_delivered_payload.is_some(),
                static_hold_refresh_interval,
            );
            delivered_frame_number = delivered_frame_number.saturating_add(1);
            continue;
        }

        let encode_source = frames.source.clone();
        let viewport = target.viewport;
        let geometry = target.geometry;
        let brightness = target.brightness;
        let frame_format = target.frame_format;
        let include_preview_jpeg = target.frame_format == DisplayFrameFormat::Jpeg
            || display_frames.read().await.has_subscriber(device_id);
        let encode_result = tokio::task::spawn_blocking(move || {
            let mut encode_state = encode_state;
            let encoded = match encode_source {
                DisplayWorkerFrameSource::Scene(scene_source) => encode_canvas_frame(
                    scene_source.as_ref(),
                    &viewport,
                    &geometry,
                    brightness,
                    frame_format,
                    include_preview_jpeg,
                    &mut encode_state,
                ),
                DisplayWorkerFrameSource::Face {
                    scene_frame,
                    face_frame,
                    blend_mode,
                    opacity,
                } => encode_face_scene_blend(
                    scene_frame.as_deref(),
                    face_frame.as_ref(),
                    &viewport,
                    &geometry,
                    brightness,
                    blend_mode,
                    opacity,
                    frame_format,
                    include_preview_jpeg,
                    &mut encode_state,
                ),
            };
            (encode_state, encoded)
        })
        .await;

        let encoded = match encode_result {
            Ok((returned_state, Ok(encoded))) => {
                encode_state = returned_state;
                encoded
            }
            Ok((returned_state, Err(error))) => {
                encode_state = returned_state;
                maybe_warn_display_error(&mut last_warned_at, &target, &error);
                continue;
            }
            Err(error) => {
                match DisplayEncodeState::new() {
                    Ok(state) => {
                        encode_state = state;
                    }
                    Err(init_error) => {
                        warn!(
                            backend_id = %backend_key,
                            device_id = %device_id,
                            error = %init_error,
                            "display worker could not recover encoder state after a blocking-task failure"
                        );
                        break;
                    }
                }
                maybe_warn_display_error(
                    &mut last_warned_at,
                    &target,
                    &anyhow!("display encode worker failed: {error}"),
                );
                continue;
            }
        };

        let payload_data = Arc::new(encoded.data);
        let payload = Arc::new(OwnedDisplayFramePayload {
            format: encoded.format,
            width: target.geometry.width,
            height: target.geometry.height,
            data: Arc::clone(&payload_data),
        });
        let preview_jpeg = match payload.format {
            DisplayFrameFormat::Jpeg => Some(Arc::clone(&payload_data)),
            DisplayFrameFormat::Rgb => encoded.preview_jpeg.map(Arc::new),
        };
        if let Some(preview_jpeg) = preview_jpeg.as_ref() {
            preview_frame_number = preview_frame_number.saturating_add(1);
            publish_display_frame_snapshot(
                &display_frames,
                device_id,
                &target.geometry,
                preview_frame_number,
                Arc::clone(preview_jpeg),
            )
            .await;
        }
        record_display_write_attempt(&display_frames, retry_attempt).await;
        let write_result = writer
            .write_display_payload_owned(device_id, Arc::clone(&payload))
            .await;
        let display_format = payload.format;
        let display_bytes = payload.data.len();
        last_delivered_payload = Some(Arc::clone(&payload));
        last_delivered_preview_jpeg = preview_jpeg;
        if let Err(error) = write_result {
            record_display_write_failure(&display_frames).await;
            maybe_warn_display_error(&mut last_warned_at, &target, &error);
            schedule_display_retry(
                &mut pending,
                &mut retry_after,
                &mut next_send_at,
                send_interval,
                static_hold_refresh_interval,
                frames,
            );
            continue;
        }
        record_display_write_success(&display_frames).await;
        last_delivered_input = Some(DisplayFrameInputState::capture(&frames, &target));
        next_hold_refresh_at = static_hold_refresh_deadline(
            &power_state,
            last_delivered_payload.is_some(),
            static_hold_refresh_interval,
        );
        delivered_frame_number = delivered_frame_number.saturating_add(1);

        trace!(
            device = %target.name,
            backend_id = %backend_key,
            device_id = %device_id,
            display_format = %display_format,
            display_bytes,
            target_fps,
            "display frame delivered"
        );

        if let Some(interval) = send_interval {
            next_send_at = advance_deadline(next_send_at, interval, Instant::now());
        }
    }

    // Drop the last-published preview so /api/v1/displays/{id}/preview.jpg
    // stops serving a stale frame after the device goes away and the JPEG
    // bytes stop being pinned in the runtime.
    display_frames.write().await.remove(device_id);
}

fn target_interval_for_fps(target_fps: u32) -> Option<Duration> {
    if target_fps == 0 {
        return None;
    }

    Some(Duration::from_secs_f64(1.0 / f64::from(target_fps)))
}

fn schedule_display_retry(
    pending: &mut Option<PendingDisplayFrame>,
    retry_after: &mut Option<Instant>,
    next_send_at: &mut Instant,
    send_interval: Option<Duration>,
    static_hold_refresh_interval: Duration,
    frames: DisplayWorkerFrameSet,
) {
    let now = Instant::now();
    let interval = send_interval.unwrap_or(static_hold_refresh_interval);
    let retry_deadline = now.checked_add(interval).unwrap_or(now);

    if send_interval.is_some() {
        *next_send_at = retry_deadline;
    } else {
        *retry_after = Some(retry_deadline);
    }
    *pending = Some(PendingDisplayFrame::retry(frames));
}

async fn record_display_write_attempt(
    display_frames: &Arc<RwLock<DisplayFrameRuntime>>,
    retry: bool,
) {
    display_frames.write().await.record_write_attempt(retry);
}

async fn record_display_write_success(display_frames: &Arc<RwLock<DisplayFrameRuntime>>) {
    display_frames.write().await.record_write_success();
}

async fn record_display_write_failure(display_frames: &Arc<RwLock<DisplayFrameRuntime>>) {
    display_frames.write().await.record_write_failure();
}

fn should_refresh_static_hold(power_state: &watch::Receiver<OutputPowerState>) -> bool {
    let state = *power_state.borrow();
    state.sleeping && state.off_output_behavior == OffOutputBehavior::Static
}

fn static_hold_refresh_deadline(
    power_state: &watch::Receiver<OutputPowerState>,
    has_cached_payload: bool,
    refresh_interval: Duration,
) -> Option<Instant> {
    if !should_refresh_static_hold(power_state) || !has_cached_payload {
        return None;
    }

    Instant::now().checked_add(refresh_interval)
}

fn maybe_warn_display_error(
    last_warned_at: &mut Option<Instant>,
    target: &DisplayTarget,
    error: &anyhow::Error,
) {
    let should_warn =
        last_warned_at.is_none_or(|last| last.elapsed() >= DISPLAY_ERROR_WARN_INTERVAL);
    if !should_warn {
        trace!(
            device = %target.name,
            backend_id = %target.backend_id,
            device_id = %target.device_id,
            error = %error,
            "suppressing repeated display write error"
        );
        return;
    }

    *last_warned_at = Some(Instant::now());
    warn!(
        device = %target.name,
        backend_id = %target.backend_id,
        device_id = %target.device_id,
        error = %error,
        "failed to push automatic display frame"
    );
}
