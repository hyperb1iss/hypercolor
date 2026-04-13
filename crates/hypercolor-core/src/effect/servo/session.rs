use std::path::Path;
use std::sync::mpsc::TryRecvError;

use anyhow::{Result, anyhow};
use hypercolor_types::canvas::Canvas;

use super::worker::{acquire_servo_worker, poison_shared_servo_worker_if_fatal};
use super::worker_client::{PendingServoFrame, ServoWorkerClient};

#[derive(Debug, Clone, Copy)]
pub struct SessionConfig {
    pub render_width: u32,
    pub render_height: u32,
    pub inject_engine_globals: bool,
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
        let session_id = worker.create_session_only(render_width, render_height)?;
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

    #[allow(dead_code)]
    pub fn load_html_file(&mut self, path: &Path) -> Result<()> {
        self.worker
            .load_effect(self.session_id, path, self.render_width, self.render_height)
    }

    pub fn request_render(&mut self, scripts: Vec<String>) -> Result<()> {
        if self.pending_render.is_some() {
            return Ok(());
        }
        self.pending_render = Some(self.worker.submit_render(
            self.session_id,
            scripts,
            self.render_width,
            self.render_height,
        )?);
        Ok(())
    }

    pub fn poll_frame(&mut self) -> Result<Option<Canvas>> {
        let Some(render) = self.pending_render.as_mut() else {
            return Ok(None);
        };

        match render.response_rx.try_recv() {
            Ok(result) => {
                self.pending_render = None;
                let canvas = result?;
                self.last_canvas = Some(canvas.clone());
                Ok(Some(canvas))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.pending_render = None;
                Err(anyhow!("Servo worker disconnected before returning a frame"))
            }
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.render_width = width.max(1);
        self.render_height = height.max(1);
    }

    #[must_use]
    pub fn last_canvas(&self) -> Option<&Canvas> {
        self.last_canvas.as_ref()
    }

    pub fn close(mut self) -> Result<()> {
        self.pending_render = None;
        self.worker.destroy_session(self.session_id)
    }
}

pub fn note_servo_session_error(context: &str, error: &anyhow::Error) {
    poison_shared_servo_worker_if_fatal(context, error);
}
