use std::time::Duration;

use anyhow::Error;
#[cfg(feature = "servo-gpu-import")]
use tracing::debug;
use tracing::warn;

#[cfg(feature = "servo-gpu-import")]
use super::super::gpu_import_backend::ServoFrameUnavailable;
use super::super::note_servo_session_error;
use super::super::telemetry::{
    record_servo_pending_render_age, record_servo_render_queue_wait_in_progress,
    record_servo_soft_stall,
};
use super::super::worker::{HARD_STALL_TIMEOUT, servo_worker_is_fatal_error};
use super::super::worker_client::ServoPendingRenderTiming;
use super::{DEFAULT_EFFECT_FPS_CAP, SOFT_STALL_FRAME_INTERVALS, ServoHardStall, ServoRenderer};
#[cfg(feature = "servo-gpu-import")]
use crate::effect::traits::EffectRenderOutput;
use crate::engine::FpsTier;
use hypercolor_types::canvas::Canvas;

impl ServoRenderer {
    pub(super) fn poll_in_flight_render(&mut self) {
        let pending = self.record_pending_render_timing();
        let soft_stall_timeout = self.soft_stall_timeout();
        let Some(session) = self.session.as_mut() else {
            return;
        };

        match session.poll_frame() {
            Ok(Some(canvas)) => self.accept_completed_canvas(canvas, false),
            Ok(None) => self.update_stall_state(pending, soft_stall_timeout),
            Err(error) => self.handle_poll_error(error),
        }
    }

    #[cfg(feature = "servo-gpu-import")]
    pub(super) fn poll_in_flight_render_output(&mut self) {
        let pending = self.record_pending_render_timing();
        let soft_stall_timeout = self.soft_stall_timeout();
        let Some(session) = self.session.as_mut() else {
            return;
        };

        match session.poll_output() {
            Ok(Some(EffectRenderOutput::Cpu(canvas))) => self.accept_completed_canvas(canvas, true),
            Ok(Some(EffectRenderOutput::Gpu(frame))) => {
                self.clear_stall_state();
                self.last_gpu_frame = Some(frame);
                self.warned_fallback_frame = false;
            }
            Ok(Some(EffectRenderOutput::Pending)) => {}
            Ok(None) => self.update_stall_state(pending, soft_stall_timeout),
            Err(error) => self.handle_poll_error(error),
        }
    }

    pub(super) fn soft_stall_timeout(&self) -> Duration {
        let tier = FpsTier::from_fps(self.active_fps_cap());
        let soft_timeout = tier
            .frame_interval()
            .mul_f32(SOFT_STALL_FRAME_INTERVALS as f32);
        soft_timeout.min(HARD_STALL_TIMEOUT)
    }

    fn record_pending_render_timing(&self) -> Option<ServoPendingRenderTiming> {
        let pending = self.session.as_ref()?.pending_render_timing()?;
        record_servo_pending_render_age(pending.total());
        if pending.executing.is_none() {
            record_servo_render_queue_wait_in_progress(self.producer_role, pending.queue_wait);
        }
        Some(pending)
    }

    pub(super) fn accept_completed_canvas(&mut self, canvas: Canvas, clear_gpu_frame: bool) {
        self.clear_stall_state();
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

    fn clear_stall_state(&mut self) {
        self.warned_stalled_frame = false;
        self.warned_queued_render = false;
        self.hard_stall = None;
    }

    /// Classifies an in-flight render that has not completed yet.
    ///
    /// Only execution time counts toward the hard stall: one worker
    /// serves every Servo session, so a render can wait behind other
    /// sessions' page loads and renders without anything being wrong
    /// with it. A long wait is reported once as a queue diagnostic and
    /// never degrades the layer. Lateness (the soft stall) still uses
    /// the whole age, because a frame stuck in the queue is just as stale.
    pub(super) fn update_stall_state(
        &mut self,
        pending: Option<ServoPendingRenderTiming>,
        soft_stall_timeout: Duration,
    ) {
        let Some(pending) = pending else {
            return;
        };
        if let Some(executing) = pending.executing
            && executing >= HARD_STALL_TIMEOUT
        {
            if self.hard_stall.is_none() {
                self.hard_stall = Some(ServoHardStall { executing });
                warn!(
                    executing_ms = executing.as_millis(),
                    queue_wait_ms = pending.queue_wait.as_millis(),
                    hard_timeout_ms = HARD_STALL_TIMEOUT.as_millis(),
                    "Servo frame render is hard-stalled; surfacing degraded state"
                );
            }
            return;
        }
        if pending.executing.is_none()
            && pending.queue_wait >= HARD_STALL_TIMEOUT
            && !self.warned_queued_render
        {
            warn!(
                producer_role = self.producer_role.as_str(),
                queue_wait_ms = pending.queue_wait.as_millis(),
                "Servo frame render is still queued behind other worker commands; reusing previous frame"
            );
            self.warned_queued_render = true;
        }

        let pending_age = pending.total();
        if self.warned_stalled_frame || pending_age < soft_stall_timeout {
            return;
        }

        record_servo_soft_stall();
        warn!(
            fps_cap = self.active_fps_cap(),
            pending_age_ms = pending_age.as_millis(),
            queue_wait_ms = pending.queue_wait.as_millis(),
            soft_timeout_ms = soft_stall_timeout.as_millis(),
            "Servo frame render is late; reusing previous frame"
        );
        self.warned_stalled_frame = true;
    }

    pub(super) fn handle_poll_error(&mut self, error: Error) {
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
