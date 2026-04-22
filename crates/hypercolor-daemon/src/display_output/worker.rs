//! Per-device display worker event loop.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::anyhow;
use tokio::sync::{RwLock, watch};
use tokio::task::JoinHandle;
use tracing::{trace, warn};

use hypercolor_core::bus::CanvasFrame;
use hypercolor_core::device::BackendIo;
use hypercolor_types::device::DeviceId;
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
use crate::display_frames::{DisplayFrameRuntime, DisplayFrameSnapshot};
use crate::render_thread::frame_pacing::advance_deadline;
use crate::session::OutputPowerState;

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

pub(super) struct DisplayWorkerHandle {
    tx: watch::Sender<Option<DisplayWorkerFrameSet>>,
    join_handle: JoinHandle<()>,
    pub config_signature: DisplayWorkerConfigSignature,
}

#[derive(Clone)]
struct PendingDisplayFrame {
    frames: DisplayWorkerFrameSet,
    force_send: bool,
}

impl PendingDisplayFrame {
    fn fresh(frames: DisplayWorkerFrameSet) -> Self {
        Self {
            frames,
            force_send: false,
        }
    }

    fn forced(frames: DisplayWorkerFrameSet) -> Self {
        Self {
            frames,
            force_send: true,
        }
    }
}

#[derive(Clone)]
struct CapturedDisplaySource {
    identity: DisplaySourceIdentity,
    snapshot: Arc<CanvasFrame>,
}

#[derive(Clone)]
enum CapturedDisplayFrameSource {
    Scene(CapturedDisplaySource),
    DirectFace(CapturedDisplaySource),
    FaceComposite {
        scene_source: CapturedDisplaySource,
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
            (Self::Scene(captured), DisplayWorkerFrameSource::Scene(frame))
            | (Self::DirectFace(captured), DisplayWorkerFrameSource::DirectFace(frame)) => {
                display_source_matches(Some(captured), Some(frame))
            }
            (
                Self::FaceComposite {
                    scene_source,
                    face_source,
                    blend_mode,
                    opacity_bits,
                },
                DisplayWorkerFrameSource::FaceComposite {
                    scene_frame,
                    face_frame,
                    blend_mode: current_blend_mode,
                    opacity,
                },
            ) => {
                display_source_matches(Some(scene_source), Some(scene_frame))
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
            DisplayWorkerFrameSource::DirectFace(frame) => Self::DirectFace(
                capture_display_source(Some(frame))
                    .expect("display worker should only capture a valid direct face frame"),
            ),
            DisplayWorkerFrameSource::FaceComposite {
                scene_frame,
                face_frame,
                blend_mode,
                opacity,
            } => Self::FaceComposite {
                scene_source: capture_display_source(Some(scene_frame))
                    .expect("display worker should only capture a valid scene composition frame"),
                face_source: capture_display_source(Some(face_frame))
                    .expect("display worker should only capture a valid face composition frame"),
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
            captured.identity == source_identity
                || Arc::ptr_eq(&captured.snapshot, source)
                || (captured.snapshot.width == source.width
                    && captured.snapshot.height == source.height
                    && captured.snapshot.rgba_bytes() == source.rgba_bytes())
        }
        _ => false,
    }
}

fn capture_display_source(source: Option<&Arc<CanvasFrame>>) -> Option<CapturedDisplaySource> {
    source.map(|source| CapturedDisplaySource {
        identity: display_source_identity(source.as_ref()),
        snapshot: Arc::clone(source),
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

impl DisplayWorkerHandle {
    #[allow(
        clippy::too_many_arguments,
        reason = "worker spawn plumbs every shared subsystem it consumes"
    )]
    pub fn spawn(
        target: Arc<DisplayTarget>,
        backend_io: BackendIo,
        power_state: watch::Receiver<OutputPowerState>,
        static_hold_refresh_interval: Duration,
        display_frames: Arc<RwLock<DisplayFrameRuntime>>,
    ) -> Self {
        let (tx, rx) = watch::channel(None::<DisplayWorkerFrameSet>);
        let worker_backend_id = target.backend_id.clone();
        let worker_device_id = target.device_id;
        let config_signature = target.worker_config_signature();
        let join_handle = tokio::spawn(run_display_worker(
            backend_io,
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
    backend_io: BackendIo,
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
    let mut last_delivered_frames = None::<DisplayWorkerFrameSet>;
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
    let mut delivered_frame_number = 0_u64;
    // Monotonic per-worker counter incremented on every preview publish so
    // repeated write failures don't reuse the same ETag for different JPEGs.
    // Decoupled from delivered_frame_number, which only advances on
    // successful device writes.
    let mut preview_frame_number = 0_u64;
    let mut last_delivered_jpeg = None::<Arc<Vec<u8>>>;

    loop {
        if pending.is_none() {
            if let Some(wake_deadline) = next_hold_refresh_at {
                tokio::select! {
                    changed = rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        pending = rx.borrow_and_update().clone().map(PendingDisplayFrame::fresh);
                    }
                    changed = power_state.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let _ = power_state.borrow_and_update();
                    }
                    () = tokio::time::sleep_until(tokio::time::Instant::from_std(wake_deadline)) => {
                        if should_refresh_static_hold(&power_state)
                            && let Some(frames) = last_delivered_frames.as_ref()
                        {
                            pending = Some(PendingDisplayFrame::forced(frames.clone()));
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
                last_delivered_frames.as_ref(),
                static_hold_refresh_interval,
            );
            continue;
        }

        if send_interval.is_some() {
            tokio::select! {
                changed = rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    pending = rx.borrow_and_update().clone().map(PendingDisplayFrame::fresh);
                    continue;
                }
                () = tokio::time::sleep_until(tokio::time::Instant::from_std(next_send_at)) => {}
            }
        }

        let Some(PendingDisplayFrame { frames, force_send }) = pending.take() else {
            continue;
        };

        let zero_brightness_output = display_brightness_factor(target.brightness) == 0;
        let input_matches = last_delivered_input
            .as_ref()
            .is_some_and(|previous| previous.matches(&frames, &target));
        if zero_brightness_output && !force_send && last_delivered_jpeg.is_some() {
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
            && let Some(jpeg) = last_delivered_jpeg.as_ref()
        {
            preview_frame_number = preview_frame_number.saturating_add(1);
            publish_display_frame_snapshot(
                &display_frames,
                device_id,
                &target.geometry,
                preview_frame_number,
                Arc::clone(jpeg),
            )
            .await;
            if let Err(error) = backend_io
                .write_display_frame_owned(device_id, Arc::clone(jpeg))
                .await
            {
                maybe_warn_display_error(&mut last_warned_at, &target, &error);
                continue;
            }
            last_delivered_frames = Some(frames);
            next_hold_refresh_at = static_hold_refresh_deadline(
                &power_state,
                last_delivered_frames.as_ref(),
                static_hold_refresh_interval,
            );
            delivered_frame_number = delivered_frame_number.saturating_add(1);
            continue;
        }

        let encode_source = frames.source.clone();
        let viewport = target.viewport;
        let geometry = target.geometry;
        let brightness = target.brightness;
        let encode_result = tokio::task::spawn_blocking(move || {
            let mut encode_state = encode_state;
            let encoded = match encode_source {
                DisplayWorkerFrameSource::Scene(scene_source) => encode_canvas_frame(
                    scene_source.as_ref(),
                    &viewport,
                    &geometry,
                    brightness,
                    &mut encode_state,
                ),
                DisplayWorkerFrameSource::DirectFace(face_source) => encode_face_scene_blend(
                    None,
                    face_source.as_ref(),
                    &viewport,
                    &geometry,
                    brightness,
                    hypercolor_types::scene::DisplayFaceBlendMode::Replace,
                    1.0,
                    &mut encode_state,
                ),
                DisplayWorkerFrameSource::FaceComposite {
                    scene_frame,
                    face_frame,
                    blend_mode,
                    opacity,
                } => encode_face_scene_blend(
                    Some(scene_frame.as_ref()),
                    face_frame.as_ref(),
                    &viewport,
                    &geometry,
                    brightness,
                    blend_mode,
                    opacity,
                    &mut encode_state,
                ),
            };
            (encode_state, encoded)
        })
        .await;

        let jpeg = match encode_result {
            Ok((returned_state, Ok(encoded))) => {
                encode_state = returned_state;
                Arc::new(encoded)
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

        let jpeg_bytes = jpeg.len();
        preview_frame_number = preview_frame_number.saturating_add(1);
        publish_display_frame_snapshot(
            &display_frames,
            device_id,
            &target.geometry,
            preview_frame_number,
            Arc::clone(&jpeg),
        )
        .await;
        let write_result = backend_io
            .write_display_frame_owned(device_id, Arc::clone(&jpeg))
            .await;
        let keep_cached_jpeg = zero_brightness_output || should_refresh_static_hold(&power_state);
        if keep_cached_jpeg {
            last_delivered_jpeg = Some(Arc::clone(&jpeg));
        } else {
            last_delivered_jpeg = None;
        }
        if !keep_cached_jpeg && let Some(reusable_jpeg) = Arc::into_inner(jpeg) {
            encode_state.jpeg_buffer = reusable_jpeg;
        }
        if let Err(error) = write_result {
            maybe_warn_display_error(&mut last_warned_at, &target, &error);
            continue;
        }
        last_delivered_input = Some(DisplayFrameInputState::capture(&frames, &target));
        last_delivered_frames = Some(frames);
        next_hold_refresh_at = static_hold_refresh_deadline(
            &power_state,
            last_delivered_frames.as_ref(),
            static_hold_refresh_interval,
        );
        delivered_frame_number = delivered_frame_number.saturating_add(1);

        trace!(
            device = %target.name,
            backend_id = %backend_key,
            device_id = %device_id,
            jpeg_bytes,
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

fn should_refresh_static_hold(power_state: &watch::Receiver<OutputPowerState>) -> bool {
    let state = *power_state.borrow();
    state.sleeping && state.off_output_behavior == OffOutputBehavior::Static
}

fn static_hold_refresh_deadline(
    power_state: &watch::Receiver<OutputPowerState>,
    last_delivered_source: Option<&DisplayWorkerFrameSet>,
    refresh_interval: Duration,
) -> Option<Instant> {
    if !should_refresh_static_hold(power_state) || last_delivered_source.is_none() {
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
