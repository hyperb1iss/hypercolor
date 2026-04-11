//! Per-device display worker event loop.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::anyhow;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{trace, warn};

use hypercolor_core::bus::CanvasFrame;
use hypercolor_core::device::BackendIo;
use hypercolor_types::device::DeviceId;
use hypercolor_types::session::OffOutputBehavior;

use super::encode::{DisplayEncodeState, display_brightness_factor, encode_canvas_frame};
use super::render::display_viewport_signature;
use super::{
    DISPLAY_ERROR_WARN_INTERVAL, DisplayGeometry, DisplayTarget, DisplayViewportSignature,
    DisplayWorkItem,
};
use crate::session::OutputPowerState;

pub(super) struct DisplayWorkerHandle {
    tx: watch::Sender<Option<DisplayWorkItem>>,
    join_handle: JoinHandle<()>,
    pub target_fps: u32,
}

#[derive(Clone, Debug)]
struct DisplayFrameInputState {
    source_identity: DisplaySourceIdentity,
    source_snapshot: Option<Arc<CanvasFrame>>,
    brightness_factor: u16,
    geometry: DisplayGeometry,
    viewport: DisplayViewportSignature,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DisplaySourceIdentity {
    generation: u64,
    storage_ptr: usize,
    width: u32,
    height: u32,
}

impl DisplaySourceIdentity {
    fn is_stable(self) -> bool {
        self.generation > 0
    }
}

impl DisplayFrameInputState {
    fn matches(&self, source: &Arc<CanvasFrame>, target: &DisplayTarget) -> bool {
        let source_identity = display_source_identity(source.as_ref());
        let source_matches = if source_identity.is_stable() {
            self.source_identity == source_identity
        } else {
            self.source_snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.rgba_bytes() == source.rgba_bytes())
        };

        source_matches
            && self.brightness_factor == display_brightness_factor(target.brightness)
            && self.geometry == target.geometry
            && self.viewport == display_viewport_signature(&target.viewport)
    }

    fn capture(source: &Arc<CanvasFrame>, target: &DisplayTarget) -> Self {
        let source_identity = display_source_identity(source.as_ref());
        Self {
            source_snapshot: (!source_identity.is_stable()).then(|| Arc::clone(source)),
            source_identity,
            brightness_factor: display_brightness_factor(target.brightness),
            geometry: target.geometry.clone(),
            viewport: display_viewport_signature(&target.viewport),
        }
    }
}

fn display_source_identity(source: &CanvasFrame) -> DisplaySourceIdentity {
    DisplaySourceIdentity {
        generation: source.surface().generation(),
        storage_ptr: source.rgba_bytes().as_ptr() as usize,
        width: source.width,
        height: source.height,
    }
}

impl DisplayWorkerHandle {
    pub fn spawn(
        target: &DisplayTarget,
        backend_io: BackendIo,
        power_state: watch::Receiver<OutputPowerState>,
        static_hold_refresh_interval: Duration,
    ) -> Self {
        let (tx, rx) = watch::channel(None::<DisplayWorkItem>);
        let worker_backend_id = target.backend_id.clone();
        let worker_device_id = target.device_id;
        let target_fps = target.target_fps;
        let join_handle = tokio::spawn(run_display_worker(
            backend_io,
            worker_backend_id,
            worker_device_id,
            target_fps,
            rx,
            power_state,
            static_hold_refresh_interval,
        ));

        Self {
            tx,
            join_handle,
            target_fps,
        }
    }

    pub fn push(&self, work: DisplayWorkItem) {
        self.tx.send_replace(Some(work));
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
async fn run_display_worker(
    backend_io: BackendIo,
    backend_key: String,
    device_id: DeviceId,
    target_fps: u32,
    mut rx: watch::Receiver<Option<DisplayWorkItem>>,
    mut power_state: watch::Receiver<OutputPowerState>,
    static_hold_refresh_interval: Duration,
) {
    let send_interval = target_interval_for_fps(target_fps);
    let mut next_send_at = Instant::now();
    let mut last_warned_at = None::<Instant>;
    let mut last_delivered_input = None::<DisplayFrameInputState>;
    let mut last_delivered_work = None::<DisplayWorkItem>;
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
    let mut pending = None::<DisplayWorkItem>;

    loop {
        if pending.is_none() {
            if let Some(hold_deadline) = next_hold_refresh_at {
                tokio::select! {
                    changed = rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        pending.clone_from(&rx.borrow_and_update());
                    }
                    changed = power_state.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let _ = power_state.borrow_and_update();
                    }
                    () = tokio::time::sleep_until(tokio::time::Instant::from_std(hold_deadline)) => {
                        if should_refresh_static_hold(&power_state)
                            && let Some(work) = last_delivered_work.as_ref() {
                            pending = Some(work.clone());
                            last_delivered_input = None;
                        }
                    }
                }
            } else {
                tokio::select! {
                    changed = rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        pending.clone_from(&rx.borrow_and_update());
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
                last_delivered_work.as_ref(),
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
                    pending.clone_from(&rx.borrow_and_update());
                    continue;
                }
                () = tokio::time::sleep_until(tokio::time::Instant::from_std(next_send_at)) => {}
            }
        }

        let Some(work) = pending.take() else {
            continue;
        };

        let source = Arc::clone(&work.source);
        let target = Arc::clone(&work.target);
        if last_delivered_input
            .as_ref()
            .is_some_and(|previous| previous.matches(&source, target.as_ref()))
        {
            trace!(
                device = %target.name,
                backend_id = %backend_key,
                device_id = %device_id,
                target_fps,
                "skipping unchanged display frame"
            );
            continue;
        }
        let encode_source = Arc::clone(&source);
        let encode_target = Arc::clone(&target);
        let encode_result = tokio::task::spawn_blocking(move || {
            let mut encode_state = encode_state;
            let encoded = encode_canvas_frame(
                encode_source.as_ref(),
                &encode_target.viewport,
                &encode_target.geometry,
                encode_target.brightness,
                &mut encode_state,
            );
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
        let write_result = backend_io
            .write_display_frame_owned(device_id, Arc::clone(&jpeg))
            .await;
        if let Some(reusable_jpeg) = Arc::into_inner(jpeg) {
            encode_state.jpeg_buffer = reusable_jpeg;
        }
        if let Err(error) = write_result {
            maybe_warn_display_error(&mut last_warned_at, target.as_ref(), &error);
            continue;
        }
        last_delivered_input = Some(DisplayFrameInputState::capture(&source, target.as_ref()));
        last_delivered_work = Some(work.clone());
        next_hold_refresh_at = static_hold_refresh_deadline(
            &power_state,
            last_delivered_work.as_ref(),
            static_hold_refresh_interval,
        );

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
}

fn target_interval_for_fps(target_fps: u32) -> Option<Duration> {
    if target_fps == 0 {
        return None;
    }

    Some(Duration::from_secs_f64(1.0 / f64::from(target_fps)))
}

fn advance_deadline(previous_deadline: Instant, interval: Duration, now: Instant) -> Instant {
    previous_deadline
        .checked_add(interval)
        .unwrap_or(now)
        .max(now)
}

fn should_refresh_static_hold(power_state: &watch::Receiver<OutputPowerState>) -> bool {
    let state = *power_state.borrow();
    state.sleeping && state.off_output_behavior == OffOutputBehavior::Static
}

fn static_hold_refresh_deadline(
    power_state: &watch::Receiver<OutputPowerState>,
    last_delivered_work: Option<&DisplayWorkItem>,
    refresh_interval: Duration,
) -> Option<Instant> {
    if !should_refresh_static_hold(power_state) || last_delivered_work.is_none() {
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
