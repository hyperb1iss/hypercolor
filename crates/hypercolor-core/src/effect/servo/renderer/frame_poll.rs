use std::time::Duration;

use anyhow::Error;
#[cfg(feature = "servo-gpu-import")]
use tracing::debug;
use tracing::warn;

#[cfg(feature = "servo-gpu-import")]
use super::super::gpu_import_backend::ServoFrameUnavailable;
use super::super::telemetry::{record_servo_pending_render_age, record_servo_soft_stall};
use super::super::worker::{RENDER_RESPONSE_TIMEOUT, servo_worker_is_fatal_error};
use super::super::{ServoSessionHandle, note_servo_session_error};
use super::{DEFAULT_EFFECT_FPS_CAP, SOFT_STALL_FRAME_INTERVALS, ServoRenderer};
#[cfg(feature = "servo-gpu-import")]
use crate::effect::traits::EffectRenderOutput;
use crate::engine::FpsTier;
use hypercolor_types::canvas::Canvas;

impl ServoRenderer {
    pub(super) fn poll_in_flight_render(&mut self) {
        let pending_age = self.record_pending_render_age();
        let soft_stall_timeout = self.soft_stall_timeout();
        let Some(session) = self.session.as_mut() else {
            return;
        };

        match session.poll_frame() {
            Ok(Some(canvas)) => self.accept_completed_canvas(canvas, false),
            Ok(None) => self.warn_if_soft_stalled(pending_age, soft_stall_timeout),
            Err(error) => self.handle_poll_error(error),
        }
    }

    #[cfg(feature = "servo-gpu-import")]
    pub(super) fn poll_in_flight_render_output(&mut self) {
        let pending_age = self.record_pending_render_age();
        let soft_stall_timeout = self.soft_stall_timeout();
        let Some(session) = self.session.as_mut() else {
            return;
        };

        match session.poll_output() {
            Ok(Some(EffectRenderOutput::Cpu(canvas))) => self.accept_completed_canvas(canvas, true),
            Ok(Some(EffectRenderOutput::Gpu(frame))) => {
                self.warned_stalled_frame = false;
                self.last_gpu_frame = Some(frame);
                self.warned_fallback_frame = false;
            }
            Ok(Some(EffectRenderOutput::Pending)) => {}
            Ok(None) => self.warn_if_soft_stalled(pending_age, soft_stall_timeout),
            Err(error) => self.handle_poll_error(error),
        }
    }

    pub(super) fn soft_stall_timeout(&self) -> Duration {
        let tier = FpsTier::from_fps(self.active_fps_cap());
        let soft_timeout = tier
            .frame_interval()
            .mul_f32(SOFT_STALL_FRAME_INTERVALS as f32);
        soft_timeout.min(RENDER_RESPONSE_TIMEOUT)
    }

    fn record_pending_render_age(&self) -> Option<Duration> {
        let pending_age = self
            .session
            .as_ref()
            .and_then(ServoSessionHandle::pending_render_age);
        if let Some(age) = pending_age {
            record_servo_pending_render_age(age);
        }
        pending_age
    }

    fn accept_completed_canvas(&mut self, canvas: Canvas, clear_gpu_frame: bool) {
        self.warned_stalled_frame = false;
        self.last_canvas = Some(canvas);
        self.clear_cached_gpu_frame_if_needed(clear_gpu_frame);
        self.warned_fallback_frame = false;
    }

    #[cfg(feature = "servo-gpu-import")]
    fn clear_cached_gpu_frame_if_needed(&mut self, clear_gpu_frame: bool) {
        if clear_gpu_frame {
            self.last_gpu_frame = None;
        }
    }

    #[cfg(not(feature = "servo-gpu-import"))]
    fn clear_cached_gpu_frame_if_needed(&mut self, _clear_gpu_frame: bool) {}

    fn warn_if_soft_stalled(
        &mut self,
        pending_age: Option<Duration>,
        soft_stall_timeout: Duration,
    ) {
        if self.warned_stalled_frame || pending_age.is_none_or(|age| age < soft_stall_timeout) {
            return;
        }

        record_servo_soft_stall();
        warn!(
            fps_cap = self.active_fps_cap(),
            pending_age_ms = pending_age.map_or(0, |age| age.as_millis()),
            soft_timeout_ms = soft_stall_timeout.as_millis(),
            "Servo frame render is late; reusing previous frame"
        );
        self.warned_stalled_frame = true;
    }

    fn handle_poll_error(&mut self, error: Error) {
        if frame_unavailable_is_logged(&error) {
            return;
        }

        note_servo_session_error("Servo frame render failed", &error);
        if servo_worker_is_fatal_error(&error) {
            self.session = None;
        }
        warn!(%error, "Servo frame render failed");
        if !self.warned_fallback_frame {
            warn!("Falling back to the previous completed frame for this effect");
            self.warned_fallback_frame = true;
        }
    }

    fn active_fps_cap(&self) -> u32 {
        self.last_animation_fps_cap
            .unwrap_or(DEFAULT_EFFECT_FPS_CAP)
    }
}

#[cfg(feature = "servo-gpu-import")]
fn frame_unavailable_is_logged(error: &Error) -> bool {
    if let Some(unavailable) = error.downcast_ref::<ServoFrameUnavailable>() {
        debug!(
            reason = unavailable.reason(),
            detail = unavailable.detail(),
            retry_ms = unavailable.retry_ms(),
            "Servo frame unavailable; reusing previous completed frame"
        );
        return true;
    }

    false
}

#[cfg(not(feature = "servo-gpu-import"))]
fn frame_unavailable_is_logged(_error: &Error) -> bool {
    false
}
