use std::path::Path;
use std::sync::mpsc::TryRecvError;
use std::time::Duration;

use anyhow::{Result, anyhow};
use hypercolor_types::canvas::Canvas;

use crate::effect::traits::EffectRenderOutput;

use super::worker::{acquire_servo_worker, poison_shared_servo_worker_if_fatal};
use super::worker_client::{
    PendingServoFrame, ServoFramePayload, ServoProducerRole, ServoRenderEnqueue, ServoRenderMode,
    ServoRenderReservation, ServoWorkerClient,
};

pub(crate) enum ServoRenderStatus {
    Submitted,
    Pending,
    Saturated,
}

pub(super) enum ServoRenderAdmission {
    Pending,
    Saturated,
    Reserved(ServoRenderReservation),
}

#[derive(Debug, Clone, Copy)]
pub struct SessionConfig {
    pub render_width: u32,
    pub render_height: u32,
    pub inject_engine_globals: bool,
    pub producer_role: ServoProducerRole,
}

pub struct ServoSessionHandle {
    worker: ServoWorkerClient,
    session_id: super::worker_client::ServoSessionId,
    render_width: u32,
    render_height: u32,
    pending_render: Option<PendingServoFrame>,
    last_canvas: Option<Canvas>,
    #[allow(dead_code)]
    inject_engine_globals: bool,
}

impl ServoSessionHandle {
    pub fn new_shared(config: SessionConfig) -> Result<Self> {
        let worker = acquire_servo_worker()?;
        Self::new(worker, config)
    }

    pub(super) fn new(worker: ServoWorkerClient, config: SessionConfig) -> Result<Self> {
        let render_width = config.render_width.max(1);
        let render_height = config.render_height.max(1);
        let session_id = worker.create_session_only_with_role(
            render_width,
            render_height,
            config.producer_role,
        )?;
        Ok(Self {
            worker,
            session_id,
            render_width,
            render_height,
            pending_render: None,
            last_canvas: None,
            inject_engine_globals: config.inject_engine_globals,
        })
    }

    pub fn load_url(&mut self, url: &str) -> Result<()> {
        self.worker
            .load_url(self.session_id, url, self.render_width, self.render_height)
    }

    pub fn load_html_file(&mut self, path: &Path) -> Result<()> {
        self.worker
            .load_effect(self.session_id, path, self.render_width, self.render_height)
    }

    pub(crate) fn request_render_cpu(&mut self, scripts: Vec<String>) -> Result<ServoRenderStatus> {
        if self.pending_render.is_some() {
            return Ok(ServoRenderStatus::Pending);
        }
        match self.worker.submit_render_with_payloads_and_mode(
            self.session_id,
            scripts,
            Vec::new(),
            self.render_width,
            self.render_height,
            ServoRenderMode::Cpu,
        )? {
            ServoRenderEnqueue::Submitted(pending) => {
                self.pending_render = Some(pending);
                Ok(ServoRenderStatus::Submitted)
            }
            ServoRenderEnqueue::Saturated => Ok(ServoRenderStatus::Saturated),
        }
    }

    pub(super) fn reserve_render(&self) -> Result<ServoRenderAdmission> {
        if self.pending_render.is_some() {
            return Ok(ServoRenderAdmission::Pending);
        }
        Ok(match self.worker.try_reserve_render(self.session_id)? {
            Some(reservation) => ServoRenderAdmission::Reserved(reservation),
            None => ServoRenderAdmission::Saturated,
        })
    }

    #[cfg(test)]
    pub(super) fn reserve_all_render_capacity(&self) -> Result<Vec<ServoRenderReservation>> {
        (0..super::worker_client::SERVO_RENDER_COMMAND_CAPACITY)
            .map(|_| {
                self.worker
                    .try_reserve_render(self.session_id)?
                    .ok_or_else(|| anyhow!("Servo render capacity exhausted before test setup"))
            })
            .collect()
    }

    pub(super) fn request_reserved_render_cpu_with_frame_payloads(
        &mut self,
        reservation: ServoRenderReservation,
        scripts: Vec<String>,
        frame_payloads: Vec<ServoFramePayload>,
    ) -> Result<()> {
        self.request_reserved_render_with_mode(
            reservation,
            scripts,
            frame_payloads,
            ServoRenderMode::Cpu,
        )
    }

    #[cfg(feature = "servo-gpu-import")]
    pub(super) fn request_reserved_render_gpu(
        &mut self,
        reservation: ServoRenderReservation,
        scripts: Vec<String>,
        frame_payloads: Vec<ServoFramePayload>,
        reuse_cached_on_no_ready: bool,
    ) -> Result<()> {
        self.request_reserved_render_with_mode(
            reservation,
            scripts,
            frame_payloads,
            ServoRenderMode::GpuPreferred {
                reuse_cached_on_no_ready,
            },
        )
    }

    fn request_reserved_render_with_mode(
        &mut self,
        reservation: ServoRenderReservation,
        scripts: Vec<String>,
        frame_payloads: Vec<ServoFramePayload>,
        mode: ServoRenderMode,
    ) -> Result<()> {
        if self.pending_render.is_some() {
            anyhow::bail!("Servo render became pending after queue admission");
        }
        let pending = self.worker.submit_reserved_render_with_payloads_and_mode(
            reservation,
            self.session_id,
            scripts,
            frame_payloads,
            self.render_width,
            self.render_height,
            mode,
        )?;
        self.pending_render = Some(pending);
        Ok(())
    }

    pub fn poll_frame(&mut self) -> Result<Option<Canvas>> {
        let Some(output) = self.poll_output()? else {
            return Ok(None);
        };
        match output {
            EffectRenderOutput::Cpu(canvas) => Ok(Some(canvas)),
            #[cfg(feature = "servo-gpu-import")]
            EffectRenderOutput::Gpu(_) => Err(anyhow!(
                "Servo worker returned a GPU frame to a CPU-only poller"
            )),
            EffectRenderOutput::Pending => Ok(None),
        }
    }

    pub fn poll_output(&mut self) -> Result<Option<EffectRenderOutput>> {
        let Some(render) = self.pending_render.as_mut() else {
            return Ok(None);
        };

        match render.response_rx.try_recv() {
            Ok(result) => {
                self.pending_render = None;
                let output = result?;
                if let Some(canvas) = output.as_cpu_canvas() {
                    self.last_canvas = Some(canvas.clone());
                }
                Ok(Some(output))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.pending_render = None;
                Err(anyhow!(
                    "Servo worker disconnected before returning a frame"
                ))
            }
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.render_width = width.max(1);
        self.render_height = height.max(1);
    }

    #[must_use]
    pub fn has_pending_render(&self) -> bool {
        self.pending_render.is_some()
    }

    #[must_use]
    pub fn pending_render_age(&self) -> Option<Duration> {
        self.pending_render
            .as_ref()
            .map(|render| render.submitted_at.elapsed())
    }

    #[must_use]
    pub fn last_canvas(&self) -> Option<&Canvas> {
        self.last_canvas.as_ref()
    }

    pub fn close(mut self) -> Result<()> {
        self.pending_render = None;
        self.worker.destroy_session(self.session_id)
    }

    pub fn close_detached(mut self) -> Result<()> {
        self.pending_render = None;
        self.worker.destroy_session_detached(self.session_id)
    }
}

pub fn note_servo_session_error(context: &str, error: &anyhow::Error) {
    poison_shared_servo_worker_if_fatal(context, error);
}
