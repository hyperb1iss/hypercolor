//! Per-device display worker event loop.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::anyhow;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{trace, warn};

use hypercolor_core::bus::CanvasFrame;
use hypercolor_core::device::BackendIo;
use hypercolor_types::canvas::PublishedSurfaceStorageIdentity;
use hypercolor_types::device::DeviceId;
use hypercolor_types::overlay::DisplayOverlayConfig;
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::session::OffOutputBehavior;

use super::encode::{
    DisplayEncodeState, display_brightness_factor, encode_canvas_frame, encode_prepared_rgb_frame,
    render_canvas_frame_rgb_into,
};
use super::overlay::{OverlayComposer, OverlayRendererFactory};
use super::render::display_viewport_signature;
use super::{
    DISPLAY_ERROR_WARN_INTERVAL, DisplayGeometry, DisplayTarget, DisplayViewportSignature,
    DisplayWorkerConfigSignature,
};
use crate::display_overlays::DisplayOverlayRuntimeRegistry;
use crate::session::OutputPowerState;

pub(super) struct DisplayWorkerHandle {
    tx: watch::Sender<Option<Arc<CanvasFrame>>>,
    join_handle: JoinHandle<()>,
    pub config_signature: DisplayWorkerConfigSignature,
}

#[derive(Clone)]
struct PendingDisplayFrame {
    source: Arc<CanvasFrame>,
    force_send: bool,
}

impl PendingDisplayFrame {
    fn fresh(source: Arc<CanvasFrame>) -> Self {
        Self {
            source,
            force_send: false,
        }
    }

    fn forced(source: Arc<CanvasFrame>) -> Self {
        Self {
            source,
            force_send: true,
        }
    }
}

#[derive(Clone, Debug)]
struct DisplayFrameInputState {
    source_identity: DisplaySourceIdentity,
    source_snapshot: Option<Arc<CanvasFrame>>,
    brightness_factor: u16,
    geometry: DisplayGeometry,
    viewport: DisplayViewportSignature,
}

#[derive(Clone, Debug)]
struct DisplayOverlayBaseState {
    source_identity: DisplaySourceIdentity,
    source_snapshot: Option<Arc<CanvasFrame>>,
    geometry: DisplayGeometry,
    viewport: DisplayViewportSignature,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DisplaySourceIdentity {
    generation: u64,
    storage: PublishedSurfaceStorageIdentity,
    width: u32,
    height: u32,
}

impl DisplayFrameInputState {
    fn matches(&self, source: &Arc<CanvasFrame>, target: &DisplayTarget) -> bool {
        let source_identity = display_source_identity(source.as_ref());
        let source_matches = self.source_identity == source_identity
            || self.source_snapshot.as_ref().is_some_and(|snapshot| {
                Arc::ptr_eq(snapshot, source)
                    || (snapshot.width == source.width
                        && snapshot.height == source.height
                        && snapshot.rgba_bytes() == source.rgba_bytes())
            });

        source_matches
            && self.brightness_factor == display_brightness_factor(target.brightness)
            && self.geometry == target.geometry
            && self.viewport == display_viewport_signature(&target.viewport)
    }

    fn capture(source: &Arc<CanvasFrame>, target: &DisplayTarget) -> Self {
        let source_identity = display_source_identity(source.as_ref());
        Self {
            source_identity,
            source_snapshot: Some(Arc::clone(source)),
            brightness_factor: display_brightness_factor(target.brightness),
            geometry: target.geometry.clone(),
            viewport: display_viewport_signature(&target.viewport),
        }
    }
}

impl DisplayOverlayBaseState {
    fn matches(&self, source: &Arc<CanvasFrame>, target: &DisplayTarget) -> bool {
        let source_identity = display_source_identity(source.as_ref());
        let source_matches = self.source_identity == source_identity
            || self.source_snapshot.as_ref().is_some_and(|snapshot| {
                Arc::ptr_eq(snapshot, source)
                    || (snapshot.width == source.width
                        && snapshot.height == source.height
                        && snapshot.rgba_bytes() == source.rgba_bytes())
            });

        source_matches
            && self.geometry == target.geometry
            && self.viewport == display_viewport_signature(&target.viewport)
    }

    fn capture(source: &Arc<CanvasFrame>, target: &DisplayTarget) -> Self {
        Self {
            source_identity: display_source_identity(source.as_ref()),
            source_snapshot: Some(Arc::clone(source)),
            geometry: target.geometry.clone(),
            viewport: display_viewport_signature(&target.viewport),
        }
    }
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
    pub fn spawn(
        target: Arc<DisplayTarget>,
        backend_io: BackendIo,
        power_state: watch::Receiver<OutputPowerState>,
        static_hold_refresh_interval: Duration,
        overlay_config_rx: watch::Receiver<Arc<DisplayOverlayConfig>>,
        overlay_runtime: Arc<DisplayOverlayRuntimeRegistry>,
        sensor_snapshot_rx: watch::Receiver<Arc<SystemSnapshot>>,
        overlay_factory: Arc<dyn OverlayRendererFactory>,
    ) -> Self {
        let (tx, rx) = watch::channel(None::<Arc<CanvasFrame>>);
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
            overlay_config_rx,
            overlay_runtime,
            sensor_snapshot_rx,
            overlay_factory,
        ));

        Self {
            tx,
            join_handle,
            config_signature,
        }
    }

    pub fn push(&self, source: Arc<CanvasFrame>) {
        self.tx.send_replace(Some(source));
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
    target: DisplayTarget,
    mut rx: watch::Receiver<Option<Arc<CanvasFrame>>>,
    mut power_state: watch::Receiver<OutputPowerState>,
    static_hold_refresh_interval: Duration,
    mut overlay_config_rx: watch::Receiver<Arc<DisplayOverlayConfig>>,
    overlay_runtime: Arc<DisplayOverlayRuntimeRegistry>,
    mut sensor_snapshot_rx: watch::Receiver<Arc<SystemSnapshot>>,
    overlay_factory: Arc<dyn OverlayRendererFactory>,
) {
    let target_fps = target.target_fps;
    let send_interval = target_interval_for_fps(target_fps);
    let mut next_send_at = Instant::now();
    let mut last_warned_at = None::<Instant>;
    let mut last_delivered_input = None::<DisplayFrameInputState>;
    let mut last_overlay_base = None::<DisplayOverlayBaseState>;
    let mut last_delivered_source = None::<Arc<CanvasFrame>>;
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
    let mut last_delivered_jpeg = None::<Arc<Vec<u8>>>;
    let mut overlay_composer = OverlayComposer::new(
        target.geometry.width,
        target.geometry.height,
        target.geometry.circular,
        overlay_factory,
    );
    let initial_overlay_config = overlay_config_rx.borrow_and_update().clone();
    overlay_composer.reconcile(initial_overlay_config.as_ref());
    publish_overlay_runtime(
        &overlay_runtime,
        device_id,
        overlay_composer.runtime_snapshot(),
    )
    .await;

    loop {
        if pending.is_none() {
            let now = Instant::now();
            let overlay_deadline = overlay_composer.next_refresh_at(now);
            let wake_deadline = earliest_deadline(next_hold_refresh_at, overlay_deadline);
            if let Some(wake_deadline) = wake_deadline {
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
                    changed = overlay_config_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let config = overlay_config_rx.borrow_and_update().clone();
                        overlay_composer.reconcile(config.as_ref());
                        publish_overlay_runtime(&overlay_runtime, device_id, overlay_composer.runtime_snapshot()).await;
                        if let Some(source) = last_delivered_source.as_ref() {
                            pending = Some(PendingDisplayFrame::forced(Arc::clone(source)));
                        }
                    }
                    changed = sensor_snapshot_rx.changed(), if overlay_composer.has_active_slots() => {
                        if changed.is_err() {
                            break;
                        }
                        let _ = sensor_snapshot_rx.borrow_and_update();
                        if let Some(source) = last_delivered_source.as_ref() {
                            pending = Some(PendingDisplayFrame::forced(Arc::clone(source)));
                        }
                    }
                    () = tokio::time::sleep_until(tokio::time::Instant::from_std(wake_deadline)) => {
                        let now = Instant::now();
                        if should_refresh_static_hold(&power_state)
                            && next_hold_refresh_at.is_some_and(|deadline| now >= deadline)
                            && let Some(source) = last_delivered_source.as_ref() {
                            pending = Some(PendingDisplayFrame::forced(Arc::clone(source)));
                        }
                        if overlay_deadline.is_some_and(|deadline| now >= deadline)
                            && let Some(source) = last_delivered_source.as_ref() {
                            pending = Some(PendingDisplayFrame::forced(Arc::clone(source)));
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
                    changed = overlay_config_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let config = overlay_config_rx.borrow_and_update().clone();
                        overlay_composer.reconcile(config.as_ref());
                        publish_overlay_runtime(&overlay_runtime, device_id, overlay_composer.runtime_snapshot()).await;
                        if let Some(source) = last_delivered_source.as_ref() {
                            pending = Some(PendingDisplayFrame::forced(Arc::clone(source)));
                        }
                    }
                    changed = sensor_snapshot_rx.changed(), if overlay_composer.has_active_slots() => {
                        if changed.is_err() {
                            break;
                        }
                        let _ = sensor_snapshot_rx.borrow_and_update();
                        if let Some(source) = last_delivered_source.as_ref() {
                            pending = Some(PendingDisplayFrame::forced(Arc::clone(source)));
                        }
                    }
                }
            }
            next_hold_refresh_at = static_hold_refresh_deadline(
                &power_state,
                last_delivered_source.as_ref(),
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
                changed = overlay_config_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let config = overlay_config_rx.borrow_and_update().clone();
                    overlay_composer.reconcile(config.as_ref());
                    publish_overlay_runtime(&overlay_runtime, device_id, overlay_composer.runtime_snapshot()).await;
                    if let Some(source) = last_delivered_source.as_ref() {
                        pending = Some(PendingDisplayFrame::forced(Arc::clone(source)));
                    }
                    continue;
                }
                changed = sensor_snapshot_rx.changed(), if overlay_composer.has_active_slots() => {
                    if changed.is_err() {
                        break;
                    }
                    let _ = sensor_snapshot_rx.borrow_and_update();
                    if let Some(source) = last_delivered_source.as_ref() {
                        pending = Some(PendingDisplayFrame::forced(Arc::clone(source)));
                    }
                    continue;
                }
                () = tokio::time::sleep_until(tokio::time::Instant::from_std(next_send_at)) => {}
            }
        }

        let Some(PendingDisplayFrame { source, force_send }) = pending.take() else {
            continue;
        };

        let input_matches = last_delivered_input
            .as_ref()
            .is_some_and(|previous| previous.matches(&source, &target));
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
            && input_matches
            && !overlay_composer.has_active_slots()
            && let Some(jpeg) = last_delivered_jpeg.as_ref()
        {
            if let Err(error) = backend_io
                .write_display_frame_owned(device_id, Arc::clone(jpeg))
                .await
            {
                maybe_warn_display_error(&mut last_warned_at, &target, &error);
                continue;
            }
            last_delivered_source = Some(source);
            next_hold_refresh_at = static_hold_refresh_deadline(
                &power_state,
                last_delivered_source.as_ref(),
                static_hold_refresh_interval,
            );
            delivered_frame_number = delivered_frame_number.saturating_add(1);
            continue;
        }
        let sensor_snapshot = Arc::clone(&sensor_snapshot_rx.borrow());
        let encode_result = if overlay_composer.has_active_slots() {
            if !last_overlay_base
                .as_ref()
                .is_some_and(|previous| previous.matches(&source, &target))
            {
                render_canvas_frame_rgb_into(
                    source.as_ref(),
                    &target.viewport,
                    &target.geometry,
                    &mut encode_state.overlay_base_rgb_buffer,
                    &mut encode_state.axis_plan,
                );
                last_overlay_base = Some(DisplayOverlayBaseState::capture(&source, &target));
            }
            if let Some(staging) = overlay_composer.compose_rgb_frame(
                &encode_state.overlay_base_rgb_buffer,
                sensor_snapshot.as_ref(),
                delivered_frame_number,
                SystemTime::now(),
                Instant::now(),
            ) {
                staging.write_into_rgb(&mut encode_state.rgb_buffer);
            }
            publish_overlay_runtime(
                &overlay_runtime,
                device_id,
                overlay_composer.runtime_snapshot(),
            )
            .await;
            let geometry = target.geometry;
            let brightness = target.brightness;
            tokio::task::spawn_blocking(move || {
                let mut encode_state = encode_state;
                let encoded = encode_prepared_rgb_frame(&geometry, brightness, &mut encode_state);
                (encode_state, encoded)
            })
            .await
        } else {
            let encode_source = Arc::clone(&source);
            let viewport = target.viewport;
            let geometry = target.geometry;
            let brightness = target.brightness;
            tokio::task::spawn_blocking(move || {
                let mut encode_state = encode_state;
                let encoded = encode_canvas_frame(
                    encode_source.as_ref(),
                    &viewport,
                    &geometry,
                    brightness,
                    &mut encode_state,
                );
                (encode_state, encoded)
            })
            .await
        };

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
                        last_overlay_base = None;
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
        let keep_cached_jpeg =
            should_refresh_static_hold(&power_state) && !overlay_composer.has_active_slots();
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
        last_delivered_input = Some(DisplayFrameInputState::capture(&source, &target));
        last_delivered_source = Some(source);
        next_hold_refresh_at = static_hold_refresh_deadline(
            &power_state,
            last_delivered_source.as_ref(),
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

    overlay_runtime.clear(device_id).await;
}

async fn publish_overlay_runtime(
    overlay_runtime: &DisplayOverlayRuntimeRegistry,
    device_id: DeviceId,
    runtime_snapshot: crate::display_overlays::DisplayOverlayRuntime,
) {
    overlay_runtime.set(device_id, runtime_snapshot).await;
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

fn earliest_deadline(left: Option<Instant>, right: Option<Instant>) -> Option<Instant> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

fn should_refresh_static_hold(power_state: &watch::Receiver<OutputPowerState>) -> bool {
    let state = *power_state.borrow();
    state.sleeping && state.off_output_behavior == OffOutputBehavior::Static
}

fn static_hold_refresh_deadline(
    power_state: &watch::Receiver<OutputPowerState>,
    last_delivered_source: Option<&Arc<CanvasFrame>>,
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
